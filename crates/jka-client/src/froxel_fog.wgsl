const FROXEL_X: u32 = 80u;
const FROXEL_Y: u32 = 45u;
const FROXEL_Z: u32 = 48u;
const FROXEL_NEAR: f32 = 4.0;
const FROXEL_FAR: f32 = 16384.0;
const CLUSTER_X: u32 = 16u;
const CLUSTER_Y: u32 = 9u;
const CLUSTER_Z: u32 = 24u;
const CLUSTER_NEAR: f32 = 1.0;
const CLUSTER_FAR: f32 = 65536.0;
const LOCAL_SHADOW_NEAR: f32 = 4.0;
const JKA_TO_GODOT_METERS: f32 = 1.0 / 64.0;

struct FroxelSettings {
    inv_view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    sun_direction_enabled: vec4<f32>,
    sun_color_intensity: vec4<f32>,
    fog_params: vec4<f32>,
    fog_color_anisotropy: vec4<f32>,
    rain_params: vec4<f32>,
    rain_occlusion: vec4<f32>,
    rain_wind: vec4<f32>,
    shadow_view_proj: array<mat4x4<f32>, 4>,
    split_depths: vec4<f32>,
    shadow_params: vec4<f32>,
    grid: vec4<u32>,
};

struct PointLight {
    position_radius: vec4<f32>,
    color_intensity: vec4<f32>,
    emitter: vec4<f32>, // xyz normal; w: 0 point, 1 one-sided area, 2 two-sided area
    shadow: vec4<f32>,
};

struct ClusterRecord {
    count: u32,
    indices: array<u32, 32>,
};

struct LightingSettings {
    values: vec4<u32>, // enabled, light count, viewport width, viewport height
    local_shadows: vec4<u32>, // enabled, shadowed count, cubemap size, reserved
    feature_flags: vec4<u32>, // area lights, voxel/probe GI, reserved, reserved
};

@group(0) @binding(0) var<uniform> settings: FroxelSettings;
@group(0) @binding(1) var<storage, read_write> integrated_fog: array<vec4<f32>>;
@group(0) @binding(2) var shadow_texture: texture_depth_2d_array;
@group(0) @binding(3) var<storage, read> dynamic_lights: array<PointLight>;
@group(0) @binding(4) var<storage, read> light_clusters: array<ClusterRecord>;
@group(0) @binding(5) var<uniform> lighting_settings: LightingSettings;
@group(0) @binding(6) var local_shadow_texture: texture_depth_cube_array;
@group(0) @binding(7) var local_shadow_sampler: sampler_comparison;
@group(0) @binding(8) var rain_occlusion_height: texture_2d<f32>;
@group(0) @binding(9) var wind_noise: texture_2d<f32>;
@group(0) @binding(10) var wind_noise_sampler: sampler;
@group(0) @binding(11) var sky_admission_texture: texture_depth_2d_array;

fn world_ray(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    var world_far = settings.inv_view_proj * ndc;
    world_far = world_far / max(abs(world_far.w), 1e-6);
    return normalize(world_far.xyz - settings.camera_pos_time.xyz);
}

fn slice_edge(slice_plus_one: u32) -> f32 {
    let z = f32(slice_plus_one) / f32(FROXEL_Z);
    return FROXEL_NEAR * pow(FROXEL_FAR / FROXEL_NEAR, z);
}

fn phase_henyey_greenstein(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let denominator = pow(max(1.0 + g2 - 2.0 * g * cos_theta, 1e-4), 1.5);
    return (1.0 - g2) / denominator;
}

fn shadow_visibility(world: vec3<f32>, camera_depth: f32) -> f32 {
    if (settings.shadow_params.z < 0.5) {
        return 1.0;
    }
    var cascade = 0u;
    if (camera_depth > settings.split_depths.x) { cascade = 1u; }
    if (camera_depth > settings.split_depths.y) { cascade = 2u; }
    if (camera_depth > settings.split_depths.z) {
        if (settings.shadow_params.w < 0.5) { return 1.0; }
        cascade = 3u;
    }
    if (camera_depth > settings.split_depths[cascade]) { return 1.0; }
    let clip = settings.shadow_view_proj[cascade] * vec4<f32>(world, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);
    if (
        uv.x <= 0.0 || uv.x >= 1.0 || uv.y <= 0.0 || uv.y >= 1.0
        || ndc.z <= 0.0 || ndc.z >= 1.0
    ) {
        return 1.0;
    }
    let map_size = max(i32(settings.shadow_params.x), 1);
    let pixel = vec2<i32>(
        clamp(i32(uv.x * f32(map_size)), 0, map_size - 1),
        clamp(i32(uv.y * f32(map_size)), 0, map_size - 1)
    );
    let stored_depth = textureLoad(shadow_texture, pixel, i32(cascade), 0);
    if (settings.grid.w > 0u) {
        let sky_depth = textureLoad(sky_admission_texture, pixel, i32(cascade), 0);
        let reverse_z = settings.shadow_params.w > 0.5;
        let clear_depth = select(1.0, 0.0, reverse_z);
        if (abs(sky_depth - clear_depth) < 0.000001) {
            return 0.0;
        }
        let receiver_depth = 0.5 + atan(ndc.z) / 3.14159265358979323846;
        let epsilon = 0.0005;
        let admitted = select(
            sky_depth <= receiver_depth + epsilon,
            sky_depth >= receiver_depth - epsilon,
            reverse_z
        );
        if (!admitted) {
            return 0.0;
        }
    }
    if (settings.shadow_params.w > 0.5) {
        return select(0.18, 1.0, ndc.z >= stored_depth);
    }
    return select(0.18, 1.0, ndc.z - settings.shadow_params.y <= stored_depth);
}

fn cluster_for_froxel(gid: vec3<u32>, camera_depth: f32) -> ClusterRecord {
    let tile_x = min(gid.x * CLUSTER_X / FROXEL_X, CLUSTER_X - 1u);
    let tile_y = min(gid.y * CLUSTER_Y / FROXEL_Y, CLUSTER_Y - 1u);
    let depth = max(camera_depth, CLUSTER_NEAR);
    let z_fraction = clamp(
        log(depth / CLUSTER_NEAR) / log(CLUSTER_FAR / CLUSTER_NEAR),
        0.0,
        0.999999
    );
    let slice = min(u32(z_fraction * f32(CLUSTER_Z)), CLUSTER_Z - 1u);
    let index = slice * CLUSTER_X * CLUSTER_Y + tile_y * CLUSTER_X + tile_x;
    return light_clusters[index];
}

fn local_shadow_visibility(light: PointLight, world: vec3<f32>) -> f32 {
    if (lighting_settings.local_shadows.x == 0u || light.shadow.x < 0.5) {
        return 1.0;
    }
    let from_light = world - light.position_radius.xyz;
    let radial_distance = length(from_light);
    let far_plane = max(light.position_radius.w, LOCAL_SHADOW_NEAR + 1.0);
    if (radial_distance <= LOCAL_SHADOW_NEAR || radial_distance >= far_plane) {
        return 1.0;
    }
    let abs_delta = abs(from_light);
    let major_distance = max(max(abs_delta.x, abs_delta.y), abs_delta.z);
    if (major_distance <= LOCAL_SHADOW_NEAR) {
        return 1.0;
    }
    let depth_ref = far_plane / (far_plane - LOCAL_SHADOW_NEAR)
        - (far_plane * LOCAL_SHADOW_NEAR)
            / ((far_plane - LOCAL_SHADOW_NEAR) * major_distance);
    let shadow_index = u32(light.shadow.x - 1.0);
    // A single comparison tap is intentional here. The integrated froxel volume
    // naturally softens the result, and avoiding 4-9 taps per local light keeps
    // volumetric lamps inexpensive enough for the FPS-first renderer.
    return textureSampleCompareLevel(
        local_shadow_texture,
        local_shadow_sampler,
        normalize(from_light),
        shadow_index,
        depth_ref - 0.0018
    );
}

fn emitter_visibility(light: PointLight, direction_to_light: vec3<f32>) -> f32 {
    if (light.emitter.w < 0.5) {
        return 1.0;
    }
    let facing = dot(normalize(light.emitter.xyz), -direction_to_light);
    if (light.emitter.w > 1.5) {
        return abs(facing);
    }
    return max(facing, 0.0);
}

fn rain_exposure(world: vec3<f32>) -> f32 {
    if (settings.rain_params.x <= 0.5) {
        return 0.0;
    }
    if (settings.rain_occlusion.z <= 0.0 || settings.rain_occlusion.w <= 0.0) {
        return 1.0;
    }
    let dimensions_u = textureDimensions(rain_occlusion_height, 0);
    if (dimensions_u.x == 0u || dimensions_u.y == 0u) {
        return 1.0;
    }
    let dimensions = vec2<f32>(f32(dimensions_u.x), f32(dimensions_u.y));
    let uv = (world.xz - settings.rain_occlusion.xy) * settings.rain_occlusion.zw;
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) {
        return 1.0;
    }
    let pixel_f = uv * dimensions;
    let pixel = vec2<i32>(
        clamp(i32(pixel_f.x), 0, i32(dimensions_u.x) - 1),
        clamp(i32(pixel_f.y), 0, i32(dimensions_u.y) - 1)
    );
    let cover_y = textureLoad(rain_occlusion_height, pixel, 0).x;
    if (cover_y <= -1.0e19) {
        return 1.0;
    }
    // Match the rain/wetness shelter boundary, but classify the froxel's AIR
    // rather than the surface eventually hit by the view ray.
    let shelter = smoothstep(6.0, 30.0, cover_y - world.y);
    return 1.0 - shelter;
}

fn rain_gust_strength(world: vec3<f32>) -> f32 {
    // The canonical GodotGrass noise field is advected by the full atmospheric
    // cloud wind. Rain/fog density does not need its own wind simulation: the
    // moving broad field is enough to make mist thicken/thin with gusts.
    let cloud_wind_jka = settings.rain_wind.xy;
    let root_m = world.xz * JKA_TO_GODOT_METERS;
    let advected_root = root_m
        - cloud_wind_jka * JKA_TO_GODOT_METERS * settings.camera_pos_time.w;
    let strength_noise = textureSampleLevel(
        wind_noise,
        wind_noise_sampler,
        advected_root * 0.025,
        0.0
    ).x;
    var strength = mix(0.25, 1.0, strength_noise);
    return strength * strength;
}

fn rain_segment_extinction(
    world: vec3<f32>,
    midpoint_depth: f32,
    segment_length: f32,
    remaining_optical_depth: f32,
) -> f32 {
    if (settings.rain_params.x <= 0.5 || remaining_optical_depth <= 0.0) {
        return 0.0;
    }
    let intensity = clamp(settings.rain_params.y, 0.0, 1.0);
    let haze_strength = clamp(settings.rain_params.z, 0.0, 1.0);
    if (haze_strength <= 0.001) {
        return 0.0;
    }
    let heavy = smoothstep(0.72, 1.0, intensity);
    let haze_start = mix(768.0, 620.0, heavy);
    // Preserve the old distant-rain onset, but soften it across froxel slices so
    // the volume never reads as a hard shell around the camera.
    let distance_ramp = smoothstep(haze_start - 160.0, haze_start + 220.0, midpoint_depth);
    if (distance_ramp <= 0.001) {
        return 0.0;
    }
    let exposure = rain_exposure(world);
    if (exposure <= 0.001) {
        return 0.0;
    }
    let base_density = mix(0.000110, 0.000335, intensity)
        * mix(1.0, 1.34, heavy)
        * haze_strength;
    let gust = rain_gust_strength(world);
    // Deliberately subtle. The field moves at global wind speed and changes the
    // local optical density by roughly +/-18%, giving visible gusts without
    // turning the rain atmosphere into rolling smoke.
    let gust_density = mix(0.82, 1.18, gust);
    let raw = base_density * distance_ramp * exposure * gust_density * segment_length;
    return min(raw, remaining_optical_depth);
}

fn rain_max_optical_depth() -> f32 {
    if (settings.rain_params.x <= 0.5) {
        return 0.0;
    }
    let intensity = clamp(settings.rain_params.y, 0.0, 1.0);
    let haze_strength = clamp(settings.rain_params.z, 0.0, 1.0);
    let heavy = smoothstep(0.72, 1.0, intensity);
    // Preserve the previous haze's distance-independent maximum rather than
    // allowing a long outdoor sightline to become fully opaque.
    let maximum = clamp(
        mix(0.31, 0.74, intensity) * mix(1.0, 1.10, heavy) * haze_strength,
        0.0,
        0.94
    );
    return -log(max(1.0 - maximum, 1.0e-4));
}

fn local_light_scattering(
    gid: vec3<u32>,
    world: vec3<f32>,
    camera_depth: f32,
    view_ray: vec3<f32>,
    fog_color: vec3<f32>,
    g: f32,
) -> vec3<f32> {
    if (lighting_settings.values.x == 0u || lighting_settings.values.y == 0u) {
        return vec3<f32>(0.0);
    }
    let cluster = cluster_for_froxel(gid, camera_depth);
    var result = vec3<f32>(0.0);
    let count = min(cluster.count, 32u);
    for (var i = 0u; i < count; i = i + 1u) {
        let light_index = cluster.indices[i];
        if (light_index >= lighting_settings.values.y || light_index >= arrayLength(&dynamic_lights)) {
            continue;
        }
        let light = dynamic_lights[light_index];
        let to_light = light.position_radius.xyz - world;
        let distance_to_light = length(to_light);
        let radius = max(light.position_radius.w, 1.0);
        if (distance_to_light <= 0.001 || distance_to_light >= radius) {
            continue;
        }
        let direction_to_light = to_light / distance_to_light;
        let falloff = max(1.0 - distance_to_light / radius, 0.0);
        let attenuation = falloff * falloff;
        let emission_visibility = emitter_visibility(light, direction_to_light);
        let phase = phase_henyey_greenstein(dot(view_ray, direction_to_light), g);
        let visibility = local_shadow_visibility(light, world);
        // JKA light entities often use values in the hundreds. Normalize that
        // authored intensity into a stable volumetric range rather than applying
        // the surface-lighting scale directly.
        let intensity = clamp(light.color_intensity.a / 300.0, 0.0, 4.0);
        result += fog_color
            * light.color_intensity.rgb
            * intensity
            * attenuation
            * emission_visibility
            * phase
            * visibility
            * 0.16;
    }
    return result;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= FROXEL_X || gid.y >= FROXEL_Y) {
        return;
    }

    let uv = vec2<f32>(
        (f32(gid.x) + 0.5) / f32(FROXEL_X),
        (f32(gid.y) + 0.5) / f32(FROXEL_Y)
    );
    let ray = world_ray(uv);
    let sun_direction = normalize(settings.sun_direction_enabled.xyz);
    let sun_color = settings.sun_color_intensity.rgb;
    let sun_intensity = clamp(settings.sun_color_intensity.w / 250.0, 0.0, 2.0);
    let g = clamp(settings.fog_color_anisotropy.w, -0.85, 0.85);
    let phase = phase_henyey_greenstein(dot(ray, -sun_direction), g);
    let fog_color = settings.fog_color_anisotropy.rgb;
    // For manual fog x is Beer-Lambert base density. For authored MAP fog
    // x is a multiplier of the exact JKA EXP2 curve.
    let density_or_authored_scale = max(settings.fog_params.x, 0.0);
    let height_falloff = max(settings.fog_params.y, 0.0);
    let height_offset = settings.fog_params.z;
    // Zero means the custom/manual Beer-Lambert volume. A positive value is
    // JKA depthForOpaque for MAP mode. OpenJK's fixed-function global fog is
    // EXP2 and is calibrated to 1/255 transmittance at that forward depth.
    let authored_depth = max(settings.fog_params.w, 0.0);
    let camera_forward = world_ray(vec2<f32>(0.5, 0.5));
    let ray_forward_scale = max(dot(ray, camera_forward), 0.0);

    var transmittance = 1.0;
    var scattering = vec3<f32>(0.0);
    var previous_depth = FROXEL_NEAR;
    var rain_optical_depth = 0.0;
    let rain_optical_depth_limit = rain_max_optical_depth();

    for (var slice = 0u; slice < FROXEL_Z; slice = slice + 1u) {
        let current_depth = slice_edge(slice + 1u);
        let midpoint_depth = 0.5 * (previous_depth + current_depth);
        let segment_length = current_depth - previous_depth;
        let world = settings.camera_pos_time.xyz + ray * midpoint_depth;
        var fog_extinction = 0.0;
        if (authored_depth > 0.001) {
            // Integrate OpenJK's EXP2 law exactly over this froxel segment:
            // T(z) = exp(-ln(255) * (z / depthForOpaque)^2). Fixed-function
            // fog uses eye-forward depth, not radial world distance.
            let previous_forward = previous_depth * ray_forward_scale;
            let current_forward = current_depth * ray_forward_scale;
            let inv_depth2 = 1.0 / max(authored_depth * authored_depth, 1e-6);
            fog_extinction = 5.5412635
                * density_or_authored_scale
                * max(
                    current_forward * current_forward
                        - previous_forward * previous_forward,
                    0.0
                )
                * inv_depth2;
        } else {
            let height_density = clamp(exp(-(world.y + height_offset) * height_falloff), 0.20, 2.5);
            let density = density_or_authored_scale * height_density;
            fog_extinction = density * segment_length;
        }

        let rain_extinction = rain_segment_extinction(
            world,
            midpoint_depth,
            segment_length,
            max(rain_optical_depth_limit - rain_optical_depth, 0.0)
        );
        rain_optical_depth += rain_extinction;
        let extinction = fog_extinction + rain_extinction;
        let step_transmittance = exp(-extinction);

        var direct_visibility = 1.0;
        if (settings.sun_direction_enabled.w > 0.5) {
            direct_visibility = shadow_visibility(world, midpoint_depth);
        }
        let rain_color = mix(
            vec3<f32>(0.39, 0.47, 0.57),
            max(sun_color, vec3<f32>(0.04)),
            0.30
        );
        var medium_color = fog_color;
        if (extinction > 1.0e-7) {
            medium_color = (fog_color * fog_extinction + rain_color * rain_extinction) / extinction;
        }
        let ambient_scatter = medium_color * 0.28;
        let sun_scatter = medium_color * sun_color * phase * direct_visibility * (0.42 * sun_intensity);
        let local_scatter = local_light_scattering(gid, world, midpoint_depth, ray, medium_color, g);
        let source = ambient_scatter + sun_scatter + local_scatter;
        let integrated_step = source * (1.0 - step_transmittance);
        scattering += transmittance * integrated_step;
        transmittance *= step_transmittance;

        let index = slice * FROXEL_X * FROXEL_Y + gid.y * FROXEL_X + gid.x;
        integrated_fog[index] = vec4<f32>(scattering, transmittance);
        previous_depth = current_depth;
    }
}
