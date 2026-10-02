const CLUSTER_X: u32 = 16u;
const CLUSTER_Y: u32 = 9u;
const CLUSTER_Z: u32 = 24u;
const CLUSTER_COUNT: u32 = CLUSTER_X * CLUSTER_Y * CLUSTER_Z;
const MAX_CLUSTER_LIGHTS: u32 = 32u;
const CLUSTER_NEAR: f32 = 1.0;
const CLUSTER_FAR: f32 = 65536.0;

struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
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
    feature_flags: vec4<u32>, // area lights, voxel/probe GI, point cluster mode: 0 off / 1 full / 2 transient-only, reserved
    map_ambient: vec4<f32>, // source-.map q3map2 ambient RGB; unused by compute/fog consumers
    map_minlight: vec4<f32>, // source-.map q3map2 minlight RGB; unused by compute/fog consumers
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<storage, read> lights: array<PointLight>;
@group(1) @binding(1) var<storage, read_write> clusters: array<ClusterRecord>;
@group(1) @binding(2) var<uniform> settings: LightingSettings;

fn z_boundary(slice: u32) -> f32 {
    let t = f32(slice) / f32(CLUSTER_Z);
    return CLUSTER_NEAR * pow(CLUSTER_FAR / CLUSTER_NEAR, t);
}

fn projected_uv(world: vec3<f32>) -> vec3<f32> {
    let clip = camera.view_proj * vec4<f32>(world, 1.0);
    if (clip.w <= 0.0001) {
        return vec3<f32>(-10.0, -10.0, -1.0);
    }
    let ndc = clip.xyz / clip.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z);
}

fn light_overlaps_tile(light: PointLight, tile_x: u32, tile_y: u32) -> bool {
    let center = light.position_radius.xyz;
    var radius = light.position_radius.w;
    if (light.emitter.w < -0.5 && light.shadow.w >= 0.5) {
        radius += length(light.emitter.xyz);
    }
    let to_light = center - camera.camera_pos_time.xyz;
    if (length(to_light) <= radius) {
        return true;
    }

    var uv_min = vec2<f32>(1.0, 1.0);
    var uv_max = vec2<f32>(0.0, 0.0);
    var any_front = false;
    let offsets = array<vec3<f32>, 6>(
        vec3<f32>(radius, 0.0, 0.0), vec3<f32>(-radius, 0.0, 0.0),
        vec3<f32>(0.0, radius, 0.0), vec3<f32>(0.0, -radius, 0.0),
        vec3<f32>(0.0, 0.0, radius), vec3<f32>(0.0, 0.0, -radius)
    );
    for (var i = 0u; i < 6u; i += 1u) {
        let projected = projected_uv(center + offsets[i]);
        if (projected.z >= 0.0) {
            any_front = true;
            uv_min = min(uv_min, projected.xy);
            uv_max = max(uv_max, projected.xy);
        }
    }
    let projected_center = projected_uv(center);
    if (projected_center.z >= 0.0) {
        any_front = true;
        uv_min = min(uv_min, projected_center.xy);
        uv_max = max(uv_max, projected_center.xy);
    }
    if (!any_front) {
        return false;
    }

    // Slightly inflate the projected bounds to keep the approximation
    // conservative near tile boundaries.
    let margin = vec2<f32>(1.0 / f32(CLUSTER_X), 1.0 / f32(CLUSTER_Y));
    uv_min = clamp(uv_min - margin, vec2<f32>(0.0), vec2<f32>(1.0));
    uv_max = clamp(uv_max + margin, vec2<f32>(0.0), vec2<f32>(1.0));
    let tile_min = vec2<f32>(f32(tile_x) / f32(CLUSTER_X), f32(tile_y) / f32(CLUSTER_Y));
    let tile_max = vec2<f32>(f32(tile_x + 1u) / f32(CLUSTER_X), f32(tile_y + 1u) / f32(CLUSTER_Y));
    return uv_max.x >= tile_min.x && uv_min.x <= tile_max.x
        && uv_max.y >= tile_min.y && uv_min.y <= tile_max.y;
}

@compute @workgroup_size(64, 1, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let cluster_index = gid.x;
    if (cluster_index >= CLUSTER_COUNT) {
        return;
    }

    clusters[cluster_index].count = 0u;
    if (settings.values.x == 0u) {
        return;
    }

    let slice_area = CLUSTER_X * CLUSTER_Y;
    let slice = cluster_index / slice_area;
    let within_slice = cluster_index - slice * slice_area;
    let tile_y = within_slice / CLUSTER_X;
    let tile_x = within_slice - tile_y * CLUSTER_X;
    let z_min = z_boundary(slice);
    let z_max = z_boundary(slice + 1u);
    let light_count = min(settings.values.y, arrayLength(&lights));

    // Runtime transient lights (sabers, blasters) live after the static map
    // lights. Visit them first so maps with many authored lights cannot fill
    // the per-cluster cap before a saber light is considered.
    let transient_start = min(settings.local_shadows.w, light_count);
    var count = 0u;
    for (var iteration = 0u; iteration < light_count; iteration += 1u) {
        let light_index = select(
            transient_start + iteration,
            iteration - (light_count - transient_start),
            iteration >= light_count - transient_start
        );
        let light = lights[light_index];
        let is_area = light.emitter.w > 0.5;
        if (is_area) {
            if (settings.feature_flags.x == 0u) {
                continue;
            }
        } else {
            let point_mode = settings.feature_flags.z;
            if (point_mode == 0u) {
                continue;
            }
            // Fast/clustered-lite keeps only runtime-authored transient lights.
            if (point_mode == 2u && light.shadow.w < 0.5) {
                continue;
            }
        }
        // A segment may illuminate receivers beyond the midpoint sphere.
        // Enclose every sampled point's influence sphere in the cluster bounds.
        var radius = light.position_radius.w;
        if (light.emitter.w < -0.5 && light.shadow.w >= 0.5) {
            radius += length(light.emitter.xyz);
        }
        let distance_to_light = distance(camera.camera_pos_time.xyz, light.position_radius.xyz);
        if (distance_to_light + radius < z_min || distance_to_light - radius > z_max) {
            continue;
        }
        if (!light_overlaps_tile(light, tile_x, tile_y)) {
            continue;
        }
        if (count < MAX_CLUSTER_LIGHTS) {
            clusters[cluster_index].indices[count] = light_index;
            count += 1u;
        }
    }
    clusters[cluster_index].count = count;
}
