// Isolated character stage: output-timeline lights, independent of BSP lighting.
fn studio_light_color(base: vec4<f32>, albedo: vec4<f32>, normal: vec3<f32>, position: vec3<f32>) -> vec4<f32> {
    let count = min(entity_lighting.values.z, 32u);
    // values.w is exclusive to the isolated Render Studio character stage.
    // With no environment selected, preserve legacy/gameplay presentation.
    if (entity_lighting.values.w == 0u && count == 0u) { return base; }
    let n = normalize(normal);
    var radiance = vec3<f32>(0.0);
    if (entity_lighting.values.w != 0u) {
        let sunlight = max(dot(n, normalize(entity_lighting.studio_sun_direction.xyz)), 0.0);
        radiance += entity_lighting.studio_ambient.rgb * entity_lighting.studio_ambient.a;
        radiance += entity_lighting.studio_sun.rgb * entity_lighting.studio_sun.a * sunlight;
    }
    for (var i = 0u; i < count; i += 1u) {
        let pr = entity_lighting.studio_lights[i * 4u];
        let ci = entity_lighting.studio_lights[i * 4u + 1u];
        let di = entity_lighting.studio_lights[i * 4u + 2u];
        let shape = entity_lighting.studio_lights[i * 4u + 3u];
        let delta = pr.xyz - position;
        let distance = max(length(delta), 0.0001);
        let direction = delta / distance;
        let attenuation = pow(max(1.0 - distance / max(pr.w, 1.0), 0.0), 2.0);
        var cone = 1.0;
        if (shape.y > 0.5) {
            let cosine = dot(-direction, di.xyz);
            let t = clamp((cosine - shape.x) / max(di.w - shape.x, 0.00001), 0.0, 1.0);
            cone = t * t * (3.0 - 2.0 * t);
        }
        radiance += ci.rgb * ci.a * attenuation * max(dot(n, direction), 0.0) * cone;
    }
    if (entity_lighting.values.w != 0u) {
        // Unlike the additive legacy path, never retain the prelit base.
        // Ambient=Sun=0, no local lights => physically black, even when the
        // renderer previously supplied fullbright vertex colors.
        return vec4<f32>(albedo.rgb * radiance, base.a);
    }
    return vec4<f32>(base.rgb + albedo.rgb * radiance, base.a);
}

// Blue Water sky world light (r_blueSkyWorldLighting) on models: the gain the
// post pass used to derive from per-triangle depth normals, here from the
// smooth interpolated normal. Ambient cube + direct come from the sky
// environment on the GPU (blue_water_scene.rs model_sky_light).
// Existing irradiance probes retain estimated original sunlight fraction in
// alpha. This is not geometric sky visibility: it is a conservative mobile-
// model proxy which preserves room lighting at the cost of under-darkening some
// outdoor shadowed models. Static BSP uses the actual traced sky exposure.
fn model_sky_authored_sun_share(world_position: vec3<f32>, n: vec3<f32>) -> f32 {
    if (entity_lighting.values.x != 2u || entity_irradiance_info.volume_enabled_pad.x < 0.5) {
        return 0.0;
    }
    let probe = (entity_irradiance_info.light_from_world * vec4<f32>(world_position, 1.0)).xyz;
    let falloff = max(entity_irradiance_info.falloff_intensity.xyz, vec3<f32>(0.0001));
    let weights = clamp((vec3<f32>(1.0) - 2.0 * abs(probe)) / (2.0 * falloff),
        vec3<f32>(0.0), vec3<f32>(1.0));
    if (min(min(weights.x, weights.y), weights.z) <= 0.0) { return 0.0; }
    let resolution = vec3<f32>(textureDimensions(entity_irradiance_volume));
    let cells = resolution / vec3<f32>(1.0, 2.0, 3.0);
    let stp = clamp((probe + vec3<f32>(0.5)) * cells, vec3<f32>(0.5), cells - vec3<f32>(0.5));
    let uvw = stp / resolution;
    let neg = select(vec3<f32>(0.0), vec3<f32>(0.5), n < vec3<f32>(0.0));
    let x = textureSampleLevel(entity_irradiance_volume, entity_irradiance_sampler,
        uvw + vec3<f32>(0.0, neg.x, 0.0), 0.0).a;
    let y = textureSampleLevel(entity_irradiance_volume, entity_irradiance_sampler,
        uvw + vec3<f32>(0.0, neg.y, 1.0 / 3.0), 0.0).a;
    let z = textureSampleLevel(entity_irradiance_volume, entity_irradiance_sampler,
        uvw + vec3<f32>(0.0, neg.z, 2.0 / 3.0), 0.0).a;
    let w = n*n;
    return clamp((x*w.x+y*w.y+z*w.z)*2.0,0.0,1.0);
}

fn sky_world_model_gain(normal: vec3<f32>, world_position: vec3<f32>) -> vec3<f32> {
    let direct = entity_lighting.sky_world[6];
    if (direct.w <= 0.0) { return vec3<f32>(1.0); }
    let n = normal * inverseSqrt(max(dot(normal, normal), 1e-12));
    let w = n * n;
    let ambient = select(entity_lighting.sky_world[1].rgb, entity_lighting.sky_world[0].rgb, n.x >= 0.0) * w.x
        + select(entity_lighting.sky_world[3].rgb, entity_lighting.sky_world[2].rgb, n.y >= 0.0) * w.y
        + select(entity_lighting.sky_world[5].rgb, entity_lighting.sky_world[4].rgb, n.z >= 0.0) * w.z;
    let incidence = max(dot(n, entity_lighting.sky_world[7].xyz), 0.0);
    var visibility = 1.0;
    if incidence > 0.0 && any(direct.rgb > vec3<f32>(0.0)) {
        visibility = model_celestial_visibility(world_position, n);
    }
    let full_gain = clamp(ambient + direct.rgb * incidence * visibility, vec3<f32>(0.03), vec3<f32>(4.0));
    var share = 1.0;
    if entity_lighting.sky_world[7].w > 0.5 {
        share = model_sky_authored_sun_share(world_position, n);
    }
    return mix(vec3<f32>(1.0), full_gain, direct.w * share);
}

// Existing JKA cascade ABI and textures, shared with the world receiver.
// Only the directional sky term is shadowed; room/ambient light stays present.
struct ModelCelestialShadows {
    view_proj: array<mat4x4<f32>, 4>, split_depths: vec4<f32>,
    light_direction_enabled: vec4<f32>, params: vec4<f32>, camera_forward: vec4<f32>,
    cascade_texel_sizes: vec4<f32>, bevy_params: vec4<f32>,
    dynamic_coverage: array<vec4<u32>, 4>,
}
@group(1) @binding(7) var model_celestial_depth: texture_depth_2d_array;
@group(1) @binding(8) var model_celestial_sampler: sampler_comparison;
@group(1) @binding(9) var<uniform> model_celestial: ModelCelestialShadows;

fn model_celestial_cascade(world: vec3<f32>, normal: vec3<f32>, index: i32) -> f32 {
    let settings = model_celestial;
    let offset = normal * (settings.bevy_params.y * settings.cascade_texel_sizes[u32(index)])
        - normalize(settings.light_direction_enabled.xyz) * settings.bevy_params.x;
    let clip = settings.view_proj[u32(index)] * vec4<f32>(world + offset, 1.0);
    if clip.w <= 0.0 { return 1.0; }
    let ndc = clip.xyz / clip.w;
    if any(abs(ndc.xy) > vec2<f32>(1.0)) || ndc.z <= 0.0 || ndc.z >= 1.0 { return 1.0; }
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    var visibility = textureSampleCompareLevel(model_celestial_depth, model_celestial_sampler, uv, index, ndc.z);
    if settings.params.w > 1.5 && textureNumLayers(model_celestial_depth) >= 9u {
        visibility = min(visibility, textureSampleCompareLevel(model_celestial_depth, model_celestial_sampler, uv, index + 5, ndc.z));
    }
    return visibility;
}
fn model_celestial_visibility(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    let settings = model_celestial;
    if settings.light_direction_enabled.w < 0.5 || textureNumLayers(model_celestial_depth) < 4u { return 1.0; }
    let depth = max(dot(world - camera.camera_pos_time.xyz, normalize(settings.camera_forward.xyz)), 0.0);
    var index = 0i;
    if depth > settings.split_depths.x { index = 1i; }
    if depth > settings.split_depths.y { index = 2i; }
    if depth > settings.split_depths.z { index = 3i; }
    if depth > settings.split_depths[u32(index)] { return 1.0; }
    var visibility = model_celestial_cascade(world, normal, index);
    if index < 3i {
        let far = settings.split_depths[u32(index)];
        let start = (1.0 - settings.bevy_params.z) * far;
        if depth >= start {
            visibility = mix(visibility, model_celestial_cascade(world, normal, index + 1i),
                clamp((depth - start) / max(far - start, 1e-5), 0.0, 1.0));
        }
    }
    return mix(1.0, visibility, clamp(settings.params.z, 0.0, 1.0));
}

// Dynamic entities use the exact irradiance-volume atlas already uploaded for
// BSP materials.  This is the Bevy/Valve ambient-cube sampler from bsp.wgsl,
// not the former fixed-direction placeholder.  Group 1 is the shared model
// material group; the three bindings are inert outside Entity Ambient mode 2.
struct ModelIrradianceVolumeSettings {
    light_from_world: mat4x4<f32>,
    falloff_intensity: vec4<f32>,
    volume_enabled_pad: vec4<f32>,
};
@group(1) @binding(4) var entity_irradiance_volume: texture_3d<f32>;
@group(1) @binding(5) var<uniform> entity_irradiance_info: ModelIrradianceVolumeSettings;
@group(1) @binding(6) var entity_irradiance_sampler: sampler;

fn model_irradiance_volume_light(world_position: vec3<f32>, normal_in: vec3<f32>) -> vec3<f32> {
    // No map / missing lightgrid: preserve the previous unlit model baseline,
    // rather than darkening every model with an empty placeholder volume.
    if (entity_irradiance_info.volume_enabled_pad.x < 0.5) {
        return vec3<f32>(1.0);
    }
    let probe_space_pos =
        (entity_irradiance_info.light_from_world * vec4<f32>(world_position, 1.0)).xyz;
    let falloff = max(entity_irradiance_info.falloff_intensity.xyz, vec3<f32>(0.0001));
    let axis_weights = clamp(
        (vec3<f32>(1.0) - 2.0 * abs(probe_space_pos)) / (2.0 * falloff),
        vec3<f32>(0.0), vec3<f32>(1.0));
    let weight = min(min(axis_weights.x, axis_weights.y), axis_weights.z);
    if (weight == 0.0) { return vec3<f32>(1.0); }

    let atlas_resolution = vec3<f32>(textureDimensions(entity_irradiance_volume));
    let resolution = atlas_resolution / vec3<f32>(1.0, 2.0, 3.0);
    // Clamp to the centers of the first/last probe: hardware trilinear filters
    // within an ambient-cube face but must not bleed into another face.
    let stp = clamp((probe_space_pos + vec3<f32>(0.5)) * resolution,
                    vec3<f32>(0.5), resolution - vec3<f32>(0.5));
    let uvw = stp / atlas_resolution;
    let n = normalize(normal_in);
    let neg_offset = select(vec3<f32>(0.0), vec3<f32>(0.5), n < vec3<f32>(0.0));
    let face_x = textureSampleLevel(entity_irradiance_volume, entity_irradiance_sampler,
                                   uvw + vec3<f32>(0.0, neg_offset.x, 0.0), 0.0);
    let face_y = textureSampleLevel(entity_irradiance_volume, entity_irradiance_sampler,
                                   uvw + vec3<f32>(0.0, neg_offset.y, 1.0 / 3.0), 0.0);
    let face_z = textureSampleLevel(entity_irradiance_volume, entity_irradiance_sampler,
                                   uvw + vec3<f32>(0.0, neg_offset.z, 2.0 / 3.0), 0.0);
    // The existing atlas alpha estimates its original sun share. Remove it
    // only while current-sky model lighting is active; room/lamp light remains.
    let remove = clamp(entity_lighting.sky_world[6].w, 0.0, 1.0);
    let rgb_x = face_x.rgb * (1.0 - face_x.a * remove);
    let rgb_y = face_y.rgb * (1.0 - face_y.a * remove);
    let rgb_z = face_z.rgb * (1.0 - face_z.a * remove);
    let nn = n * n;
    // For one map volume the Bevy weight normalization cancels. Match BSP's
    // intensity without clipping bright probes: irradiance is lighting, not
    // a display color constrained to [0, 1].
    return max((rgb_x * nn.x + rgb_y * nn.y + rgb_z * nn.z)
               * entity_irradiance_info.falloff_intensity.w, vec3<f32>(0.0));
}
