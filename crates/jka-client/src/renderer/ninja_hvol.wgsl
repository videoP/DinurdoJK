// UE 5.6 Niagara Heterogeneous Volumes live shading, shared by every Fluid
// Ninja HVOL instance (Fog_Mist's HVOL 32m, Fire_Smoke_Explosive's Smoke2).
// The host cache uses DinurdoJK's map lighting and opaque shadow maps.
//
// The instance prelude (included before this stage) defines the volume:
//   fn hv_box_centre() -> vec3<f32>          map units
//   fn hv_box_axes() -> mat3x3<f32>          orthonormal local X/Y/Z in world
//   fn hv_box_half_extent() -> vec3<f32>     map units along each local axis
//   fn hv_box_resolution() -> vec3<f32>      voxel counts along each local axis
//   fn hv_max_steps() -> f32
//   fn hv_max_distance() -> f32              map units along the view axis
//   fn hv_support(world: vec3<f32>, pad: f32) -> bool
//   fn hv_sample(world, eye, ray_dx, ray_dy) -> HvSample (per-cm coefficients)
struct HvSample {
    extinction: vec3<f32>,
    emission: vec3<f32>,
    albedo: vec3<f32>,
};

@group(2) @binding(0) var<uniform> nv: NinjaVolumeParams;
@group(2) @binding(1) var hv_depth: texture_2d<f32>;
@group(2) @binding(2) var hv_blue_noise: texture_2d<f32>;
@group(2) @binding(3) var hv_cache_write: texture_storage_3d<rgba16float, write>;
@group(2) @binding(4) var hv_cache: texture_3d<f32>;
@group(2) @binding(5) var hv_radiance: texture_storage_2d<rgba16float, write>;
@group(2) @binding(6) var hv_cache_sampler: sampler;

const HV_ISOTROPIC_PHASE: f32 = 1.0 / (4.0 * 3.141592653589793);
const HV_CM_PER_MAP_UNIT: f32 = 2.54;

fn hv_to_local(world: vec3<f32>) -> vec3<f32> {
    return (world - hv_box_centre()) * hv_box_axes();
}

fn hv_from_uvw(uvw: vec3<f32>) -> vec3<f32> {
    return hv_box_centre() + hv_box_axes() * ((2.0 * uvw - 1.0) * hv_box_half_extent());
}

// The ordinary weather-fog path normalizes JKA map-light photon strengths by
// 300. Gameplay FX (including sabers) arrive with an intensity near 1 instead.
// Keep the shared cluster/shadow/falloff machinery, but normalize sources by
// their existing GPU provenance flag so an FX light is not 300x too dim here.
// This is HVOL-only: it does not alter the weather fog or BSP shaders.
fn hv_local_light_scattering(froxel: vec3<u32>, world: vec3<f32>, depth: f32, ray: vec3<f32>) -> vec3<f32> {
    // Without area lights, a point mode or the HVOL transient bit, no cluster
    // pass ran this frame and the list is stale.
    let flags = lighting_settings.feature_flags;
    if (lighting_settings.values.x == 0u || lighting_settings.values.y == 0u
        || (flags.x == 0u && (flags.z & 0xffu) == 0u && (flags.z & 0x1000u) == 0u)) {
        return vec3<f32>(0.0);
    }
    let cluster = cluster_for_froxel(froxel, depth);
    var result = vec3<f32>(0.0);
    for (var i = 0u; i < min(cluster.count, 32u); i++) {
        let index = cluster.indices[i];
        if (index >= lighting_settings.values.y || index >= arrayLength(&dynamic_lights)) {
            continue;
        }
        let light = dynamic_lights[index];
        if (light.shape.w < -0.5 || light.shadow.y > 0.5) {
            continue;
        }
        var source = light.position_radius.xyz;
        // RT saber lights are real blade segments, not midpoint point lights.
        if (light.emitter.w < -0.5 && light.shadow.w >= 0.5) {
            let half = light.emitter.xyz;
            let projection = clamp(dot(world - source, half) / max(dot(half, half), 1.0e-6), -1.0, 1.0);
            source += projection * half;
        }
        let to_light = source - world;
        let distance_to_light = length(to_light);
        if (distance_to_light <= 0.001 || distance_to_light >= max(light.position_radius.w, 1.0)) {
            continue;
        }
        let toward_light = to_light / distance_to_light;
        // Forward+/RT surface lighting applies a 2.43 gain to ordinary FX
        // uploads. Undo that receiver-specific gain for consistent HVOL light
        // strength across Legacy, Vertex, Lite, Forward+ and RT. Radial Ninja
        // sources deliberately bypass that gain during upload.
        let fx_mode = (flags.z & 0x2000u) != 0u;
        let scaled_fx_intensity = light.color_intensity.a * select(
            1.0, 1.0 / 2.43, fx_mode && light.shadow.y > -0.001
        );
        let intensity = select(
            clamp(light.color_intensity.a / 300.0, 0.0, 4.0),
            clamp(scaled_fx_intensity, 0.0, 4.0),
            light.shadow.w >= 0.5
        );
        result += light.color_intensity.rgb * intensity
            * local_light_attenuation(light, distance_to_light)
            * emitter_visibility(light, toward_light)
            * local_shadow_visibility(light, world)
            * phase_henyey_greenstein(dot(ray, toward_light), 0.0)
            * 0.16;
    }
    return result;
}

@compute @workgroup_size(4, 4, 4)
fn hv_build_lighting_cache(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(hv_cache_write);
    if (any(gid >= size)) {
        return;
    }
    let uvw = (vec3<f32>(gid) + 0.5) / vec3<f32>(size);
    let world = hv_from_uvw(uvw);
    var light = vec3<f32>(0.0);
    // A trilinear sample needs its neighbouring cache points as well.
    let pad = 2.0 * hv_box_half_extent().y / f32(size.y);
    if (hv_support(world, pad)) {
        let to_point = world - nv.view.xyz;
        let radial = length(to_point);
        let ray = to_point / max(radial, 1.0e-4);
        if (settings.sun_direction_enabled.w > 0.5) {
            let intensity = clamp(settings.sun_color_intensity.w / 250.0, 0.0, 2.0);
            light += settings.sun_color_intensity.rgb * shadow_visibility(world, radial) * (0.42 * intensity);
        }
        // Project using the host view that built the local-light clusters.
        let clip = nv.view_proj * vec4<f32>(world, 1.0);
        if (clip.w > 0.0) {
            let screen = clamp(clip.xy / clip.w * vec2<f32>(0.5, -0.5) + 0.5,
                vec2<f32>(0.0), vec2<f32>(0.99999));
            let cell = vec2<u32>(screen * vec2<f32>(f32(FROXEL_X), f32(FROXEL_Y)));
            light += hv_local_light_scattering(vec3<u32>(cell, 0u), world, radial, ray)
                / HV_ISOTROPIC_PHASE;
        }
    }
    textureStore(hv_cache_write, gid, vec4<f32>(light, 0.0));
}

@compute @workgroup_size(8, 8, 1)
fn hv_render(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(hv_radiance);
    if (any(gid.xy >= size)) {
        return;
    }
    let pixel = gid.xy * nv.frame.y;
    let uv = (vec2<f32>(pixel) + 0.5) / nv.misc.xy;
    let depth_size = textureDimensions(hv_depth);
    let depth_pixel = min(vec2<u32>(uv * vec2<f32>(depth_size)), depth_size - 1u);
    let ray = nv_world_ray(uv);
    let eye = nv.view.xyz;
    let radial = textureLoad(hv_depth, depth_pixel, 0).x;
    let limit = min(radial, hv_max_distance() / dot(ray, nv_forward()));
    // Slab test in the box's local frame (orthonormal axes keep t in map units).
    let half = hv_box_half_extent();
    let bounds = fog_ray_box(hv_to_local(eye), ray * hv_box_axes(), -half, half);
    let entry = max(bounds.x, 0.0);
    let end = min(bounds.y, limit);
    var radiance = vec3<f32>(0.0);
    var transmittance = vec3<f32>(1.0);
    if (end > entry) {
        // CalcStepSize, RecalculateStepSize and CalcStepCount.
        let local_ray = ray * hv_box_axes();
        let candidates = half / (hv_box_resolution() * max(abs(local_ray), vec3<f32>(1.0e-8)));
        var step = max(min(candidates.x, min(candidates.y, candidates.z)), 0.01 / HV_CM_PER_MAP_UNIT);
        let diagonal = length(2.0 * half);
        if u32(diagonal / step) > u32(hv_max_steps()) {
            step = diagonal / hv_max_steps();
        }
        let span = end - entry;
        let count = max(u32(ceil(span / step)), 1u);
        let blue_coord = vec2<u32>(pixel.x & 127u, (nv.frame.x & 7u) * 128u + (pixel.y & 127u));
        let jitter = textureLoad(hv_blue_noise, blue_coord, 0).r;
        let ray_dx = nv_world_ray(uv + vec2<f32>(1.0 / nv.misc.x, 0.0)) - ray;
        let ray_dy = nv_world_ray(uv + vec2<f32>(0.0, 1.0 / nv.misc.y)) - ray;
        for (var i = 0u; i < count; i++) {
            let hit = entry + step * (jitter + f32(i));
            let world = eye + ray * hit;
            if (!hv_support(world, 0.0)) {
                continue;
            }
            let sample = hv_sample(world, eye, ray_dx * hit, ray_dy * hit);
            let local_uv = 0.5 * hv_to_local(world) / half + 0.5;
            let light = textureSampleLevel(hv_cache, hv_cache_sampler, local_uv, 0.0).rgb;
            // RayMarchSingleScattering integrates only the remaining span on the last step.
            let step_cm = select(step, span - step * f32(i), i == count - 1u) * HV_CM_PER_MAP_UNIT;
            radiance += (sample.emission + light * sample.albedo * sample.extinction * HV_ISOTROPIC_PHASE)
                * (step_cm * transmittance);
            transmittance *= exp(-sample.extinction * step_cm);
            if (all(transmittance < vec3<f32>(1.0e-7))) {
                transmittance = vec3<f32>(0.0);
                break;
            }
        }
    }
    // HeterogeneousVolumesLiveShadingPipeline writes Common.ush Luminance.
    let luminance = dot(transmittance, vec3<f32>(0.212639, 0.715169, 0.0721923));
    textureStore(hv_radiance, gid.xy, vec4<f32>(radiance, luminance));
}
