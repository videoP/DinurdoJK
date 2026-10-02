@group(0) @binding(0) var<uniform> settings: PostSettings;
@group(0) @binding(1) var linear_depth_texture: texture_2d<f32>;
@group(0) @binding(2) var rain_occlusion_height: texture_2d<f32>;
@group(0) @binding(3) var rain_haze_mask_out: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var wind_noise: texture_2d<f32>;
@group(0) @binding(5) var wind_noise_sampler: sampler;

const JKA_TO_GODOT_METERS: f32 = 1.0 / 64.0;

// Gust density is intentionally much coarser than the already-low-resolution
// haze mask: one source-wind lookup feeds a whole 8x8 compute tile.
var<workgroup> shared_gust: f32;

fn valid_depth(depth: f32) -> bool {
    return depth > 0.0 && depth < 999999.0;
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

fn godot_grass_wind_strength(world: vec3<f32>) -> f32 {
    // Same gust-strength field used by grass/rain. The field itself travels at
    // full volumetric-cloud wind speed; only the physical response of grass/rain
    // is reduced. This remains one lookup per 8x8 low-resolution haze tile.
    let prevailing_angle = settings.cloud_layer.w;
    let prevailing_dir = vec2<f32>(cos(prevailing_angle), sin(prevailing_angle));
    let cloud_speed_m = max(settings.cloud_layer.z, 0.0) * JKA_TO_GODOT_METERS;
    let root_m = world.xz * JKA_TO_GODOT_METERS;
    let advected_root = root_m
        - prevailing_dir * cloud_speed_m * settings.camera_pos_time.w;
    let strength_noise = textureSampleLevel(
        wind_noise,
        wind_noise_sampler,
        advected_root * 0.025,
        0.0
    ).x;
    var strength = mix(0.25, 1.0, strength_noise);
    return strength * strength;
}

fn rain_haze_point_exposure(world: vec3<f32>) -> f32 {
    // The rain blocker is deliberately a top-down field: vertical walls should
    // not stop vertically falling rain. For haze, sample AIR with this field
    // rather than trying to classify the visible surface itself.
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
    let shelter = smoothstep(6.0, 30.0, cover_y - world.y);
    return 1.0 - shelter;
}

fn rain_haze_exposure_along_ray(ray: vec3<f32>, depth: f32) -> f32 {
    // Haze is a property of the air between the eye and the surface, not of the
    // brush hit by the depth buffer. Four longitudinal samples cost less than
    // the old endpoint + four-neighbour test and avoid vertical-wall ambiguity.
    // Keep the final sample slightly in front of the visible surface so a wall
    // never gets classified from the room on its far side.
    let surface_margin = min(12.0, depth * 0.06);
    let final_depth = max(depth - surface_margin, 0.0);
    let camera = settings.camera_pos_time.xyz;
    let sample_depths = array<f32, 4>(
        depth * 0.18,
        depth * 0.45,
        depth * 0.72,
        final_depth
    );

    var exposure = 0.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        exposure = exposure + rain_haze_point_exposure(camera + ray * sample_depths[i]);
    }
    return exposure * 0.25;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
    @builtin(workgroup_id) workgroup_id: vec3<u32>
) {
    let mask_dims = textureDimensions(rain_haze_mask_out);

    // One GodotGrass gust-strength lookup for the whole 8x8 haze tile. All
    // invocations reach the barrier before tail lanes return.
    if (local_index == 0u) {
        shared_gust = 0.5;
        if (settings.rain.x > 0.5 && mask_dims.x > 0u && mask_dims.y > 0u) {
            let centre_gid = min(
                workgroup_id.xy * vec2<u32>(8u, 8u) + vec2<u32>(4u, 4u),
                mask_dims - vec2<u32>(1u, 1u)
            );
            let centre_uv = (vec2<f32>(f32(centre_gid.x), f32(centre_gid.y)) + vec2<f32>(0.5))
                / vec2<f32>(f32(mask_dims.x), f32(mask_dims.y));
            let depth_dims = textureDimensions(linear_depth_texture, 0);
            if (depth_dims.x > 0u && depth_dims.y > 0u) {
                let max_pixel = vec2<i32>(i32(depth_dims.x) - 1, i32(depth_dims.y) - 1);
                let pixel = clamp(
                    vec2<i32>(
                        i32(clamp(centre_uv.x, 0.0, 0.999999) * f32(depth_dims.x)),
                        i32(clamp(centre_uv.y, 0.0, 0.999999) * f32(depth_dims.y))
                    ),
                    vec2<i32>(0), max_pixel
                );
                let depth = textureLoad(linear_depth_texture, pixel, 0).x;
                if (valid_depth(depth)) {
                    shared_gust = godot_grass_wind_strength(world_position(centre_uv, depth));
                }
            }
        }
    }
    workgroupBarrier();

    if (gid.x >= mask_dims.x || gid.y >= mask_dims.y) {
        return;
    }
    var exposure = 1.0;
    if (settings.rain.x > 0.5) {
        let uv = (vec2<f32>(f32(gid.x), f32(gid.y)) + vec2<f32>(0.5))
            / vec2<f32>(f32(mask_dims.x), f32(mask_dims.y));
        let depth_dims = textureDimensions(linear_depth_texture, 0);
        let max_pixel = vec2<i32>(i32(depth_dims.x) - 1, i32(depth_dims.y) - 1);
        let pixel = clamp(
            vec2<i32>(
                i32(clamp(uv.x, 0.0, 0.999999) * f32(depth_dims.x)),
                i32(clamp(uv.y, 0.0, 0.999999) * f32(depth_dims.y))
            ),
            vec2<i32>(0), max_pixel
        );
        let depth = textureLoad(linear_depth_texture, pixel, 0).x;
        if (valid_depth(depth)) {
            exposure = rain_haze_exposure_along_ray(world_ray(uv), depth);
        }
    }
    // R = shelter/exposure, G = coarse shared GodotGrass gust strength. Packing
    // both into the mask lets full-resolution post reuse its existing one sample.
    textureStore(
        rain_haze_mask_out,
        vec2<i32>(i32(gid.x), i32(gid.y)),
        vec4<f32>(exposure, shared_gust, 0.0, 1.0)
    );
}
