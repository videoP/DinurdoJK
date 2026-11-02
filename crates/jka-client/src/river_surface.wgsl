// Single Layer Water base pass for the live river: BasePassPixelShader.usf with
// FULL shading quality, camera above water, deferred (reflections belong to the
// separate SingleLayerWaterComposite pass). Copyright Epic Games, Inc. All Rights
// Reserved. Composed after the river material/wave/surface/foam/SLW modules
// and the generated settings; see river_surface.rs for what is not ported yet.
struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
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
struct RiverBody {
    origin: vec4<f32>, // native actor origin (XY centre, minimum Z), cm
    grid: vec4<f32>,   // world extent XY in cm, mesh cell count XY
};
// Per frame, in river axes (JKA/Unreal Z-up) and Unreal centimetres.
struct RiverView {
    eye: vec4<f32>,             // xyz eye, w projection far plane (map units)
    forward: vec4<f32>,         // view forward, w projection near plane (map units)
    right: vec4<f32>,           // screen right
    up: vec4<f32>,              // screen up
    distortion: vec4<f32>,      // SingleLayerWater.DistortionParams
    sun_direction: vec4<f32>,   // DirectionalLightDirection (toward the light)
    sun_illuminance: vec4<f32>, // rgb DirectionalLightColor * pi; w 1 = Blue Water ambient
    time: vec4<f32>,            // material Time, foam detail present, Engine.DeltaTime
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<uniform> body: RiverBody;
@group(1) @binding(1) var sim_grid: texture_2d<f32>;
@group(1) @binding(2) var normals_rt: texture_2d<f32>;
@group(1) @binding(3) var normal_sampler: sampler;
@group(1) @binding(4) var ambient_texture: texture_2d<f32>;
@group(1) @binding(5) var directed_texture: texture_2d<f32>;
@group(1) @binding(6) var ambient_sampler: sampler;
@group(1) @binding(7) var foam_rt: texture_2d<f32>;
@group(1) @binding(8) var foam_rt_sampler: sampler;
@group(2) @binding(0) var shadow_texture: texture_depth_2d_array;
@group(2) @binding(1) var shadow_sampler: sampler_comparison;
@group(2) @binding(2) var<uniform> shadow_settings: ShadowSettings;
@group(2) @binding(7) var sky_admission_texture: texture_depth_2d_array;
@group(3) @binding(0) var<uniform> river_view: RiverView;
@group(3) @binding(1) var scene_color: texture_2d<f32>;
@group(3) @binding(2) var scene_depth: texture_depth_2d;
@group(3) @binding(3) var scene_sampler: sampler;
@group(3) @binding(4) var preintegrated_gf: texture_2d<f32>;
@group(3) @binding(5) var gf_sampler: sampler;
@group(3) @binding(6) var ocean_detail: texture_2d<f32>;
@group(3) @binding(7) var ocean_sampler: sampler;
@group(3) @binding(8) var foam_detail: texture_2d<f32>;
@group(3) @binding(9) var foam_detail_sampler: sampler;
@group(4) @binding(0) var env_map: texture_2d<f32>;
@group(4) @binding(1) var env_sampler: sampler;
@group(4) @binding(2) var<uniform> env: SkyEnvironment;

struct RiverAuthored {water:RiverSlwParameters,foam:RiverFoamSettings,detail:vec4<f32>,underwater:vec4<f32>,underwater_depth:vec4<f32>}
@group(1) @binding(9) var<uniform> authored: RiverAuthored;
@group(3) @binding(11) var reflection_hzb: texture_2d<f32>;

const RIVER_CM_PER_UNIT: f32 = 2.5;

// The world's cascaded sun shadow, as grass.wgsl receives it (opaque casters
// and sky admission; no thin-glass transmission). Shadow maps have one mip, so
// the explicit-level compare matches bsp.wgsl's implicit one.
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
            visibility += textureSampleCompareLevel(shadow_texture, shadow_sampler, uv + offset, cascade, ndc.z - bias);
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
    if shadow_settings.params.w > 1.5 {
        return sample_shadow_map_bevy_castano(uv, ndc.z, cascade+5i);
    }
    return min(sample_shadow_map_bevy_castano(uv, ndc.z, cascade), sample_shadow_map_bevy_castano(uv, ndc.z, cascade+5i));
}

fn shader_sun_sky_admission(world_position: vec3<f32>, cascade: i32) -> f32 {
    if shadow_settings.params.w > 1.5 { return 1.0; }
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

struct RiverOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>, // river axes, cm
    @location(2) @interpolate(flat) eye_water: vec4<f32>, // surface, bed, wet flag
};

// Live-grid WPO: SimGridRT height plus the advected FFT wave height on the
// reset grid mesh (unrotated actor: local Z is world Z).
@vertex fn vs(@location(0) position: vec3<f32>, @location(3) uv: vec2<f32>) -> RiverOut {
    let up = vec3<f32>(0.0, 0.0, 1.0);
    let camera_distance = distance(body.origin.xyz + position, river_view.eye.xyz);
    let world = body.origin.xyz + river_live_grid_wpo(sim_grid, normals_rt, normal_sampler, ocean_detail, ocean_sampler, uv,
        body.origin.xy + position.xy, camera_distance, river_view.time.x, position, up, river_motion(river_view.time.z),
        river_wpo_advection(), river_wave_height(), false);
    let map = world / RIVER_CM_PER_UNIT;
    var out: RiverOut;
    out.clip = camera.view_proj * vec4<f32>(map.x, map.z, -map.y, 1.0);
    out.uv = uv;
    out.world = world;
    out.eye_water = river_eye_water();
    return out;
}

// ConvertFromDeviceZ for the finite reversed-Z projection, in cm.
fn river_scene_depth(device_z: f32) -> f32 {
    let near = river_view.forward.w;
    let far = river_view.eye.w;
    return near * far / (device_z * (far - near) + near) * RIVER_CM_PER_UNIT;
}

// SampleSceneDepthWithoutWaterBilateral. The refraction-mask pass is not
// ported: every texel reads as water with a full refraction mask.
fn river_gather_behind(uv: vec2<f32>, size: vec2<f32>, water_depth: f32, slope: f32) -> RiverSlwBilateral {
    let z = textureGather(scene_depth, scene_sampler, uv + (0.5 / 256.0) / size);
    let depth = vec4<f32>(river_scene_depth(z.x), river_scene_depth(z.y), river_scene_depth(z.z), river_scene_depth(z.w));
    return river_slw_bilateral(uv, size, depth, vec4<f32>(1.0), water_depth, slope);
}

// TransformWorldVectorToView: x screen right, y screen up, z forward.
fn river_to_view(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(v, river_view.right.xyz), dot(v, river_view.up.xyz), dot(v, river_view.forward.xyz));
}

@fragment fn fs(input: RiverOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let eye = river_view.eye.xyz;
    let pixel_depth = dot(input.world - eye, river_view.forward.xyz); // GetPixelDepth
    // The bilateral slope tolerance (ddx_fine/ddy_fine), taken before the clip.
    let slope = max(abs(dpdxFine(pixel_depth)), abs(dpdyFine(pixel_depth)));

    // OpacityMask: attribute default 1, Opacity 1, SingleRiverRendering; JKA
    // has no CustomDepth, so it reads as cleared (beyond every pixel).
    let fluid = river_live_field(sim_grid, input.uv);
    let masks = river_surface_masks(fluid.y, 1.0, input.uv, vec2<f32>(textureDimensions(sim_grid, 0)), 1.0, pixel_depth, 3.0e38);

    // Primary foam and the detail-normal graph, both before the clip: their
    // advected samples take fragment derivatives. Without the foam detail
    // resource foam is absent (not substituted): zero total and opacity.
    let world_xy = input.world.xy;
    let time = river_view.time.x;
    var foam = RiverFoam(vec3<f32>(0.0, 0.0, 1.0), 0.0, 0.0, 0.0, 0.0, vec3<f32>(0.0));
    if river_view.time.y > 0.5 {
        foam = river_live_grid_foam(sim_grid, normals_rt, normal_sampler, foam_rt, foam_rt_sampler, foam_detail, foam_detail_sampler,
            input.uv, world_xy, time, river_foam_advection(), river_foam_settings());
    }
    // FoamOpacity 1 (live instance); the normal gate takes the simulation mask.
    let graph_normal = river_live_grid_normal(sim_grid, normals_rt, normal_sampler, ocean_detail, ocean_sampler, input.uv, world_xy, time,
        river_motion(river_view.time.z), river_wpo_advection(), river_detail_advection(), river_wave_height(),
        foam.normal, foam.total, river_foam_settings().shading.x, masks.simulation_mask);
    if masks.material_mask < 0.3333 {
        discard;
    }

    // Hybrid-style underside replaces the above-water-only SLW volume path.
    // The completed scene receives absorption once, in the submerged pass.
    if input.eye_water.z > 0.5 {
        return river_hybrid_underside(input, graph_normal, foam);
    }

    // Two-sided: WorldNormal flips on the back face.
    let two_sided_sign = select(-1.0, 1.0, front);
    let n = graph_normal * two_sided_sign;
    let v = normalize(eye - input.world);
    let n_dot_v = dot(n, v);
    let p = river_slw_parameters();
    let surface = river_slw_surface(p, n_dot_v, two_sided_sign, pixel_depth);
    // WaterAttributes, Water_Underside, then MF_ShallowWaterRiver's foam lerps.
    let attributes = river_foam_attributes(
        RiverFoamAttributes(surface.base_color, surface.roughness, surface.specular, surface.opacity), foam, river_foam_settings());
    // The foam scattering only enters through WaterInfo velocity, which is
    // zero for spline-less (simulation-first) rivers.
    let coefficients = river_slw_coefficients(p, foam.scattering, vec2<f32>(0.0), two_sided_sign);

    let roughness = attributes.roughness;
    let specular = attributes.specular;
    let coverage = saturate(attributes.opacity); // GetMaterialOpacity -> BaseMaterialCoverageOverWater
    let water_visibility = 1.0 - coverage;
    let specular_color = vec3<f32>(0.08 * specular); // ComputeF0, metallic 0
    let diffuse_color = attributes.base_color * (1.0 - surface.metallic) * coverage;
    let gf = textureSampleLevel(preintegrated_gf, gf_sampler, vec2<f32>(max(0.0, n_dot_v), roughness), 0.0).rg;
    let env_brdf = river_slw_env_brdf(specular_color, gf);

    // DiffuseIndirectLighting follows the active sky.
    var indirect: vec3<f32>;
    if river_view.sun_illuminance.w > 0.5 {
        indirect = river_slw_sky_indirect(env_irradiance(env_map, env_sampler, env, river_slw_render_vector(n)));
    } else {
        let ambient = textureSampleLevel(ambient_texture, ambient_sampler, input.uv, 0.0);
        let directed = textureSampleLevel(directed_texture, ambient_sampler, input.uv, 0.0);
        indirect = river_slw_classic_indirect(ambient, directed, n);
    }

    // Forward directional light (DefaultLitBxDF), light colour E times shadow.
    let l = river_view.sun_direction.xyz;
    let sun = river_view.sun_illuminance.rgb;
    let map_position = input.world / RIVER_CM_PER_UNIT;
    let shadow = cascaded_shadow_visibility(
        vec3<f32>(map_position.x, map_position.z, -map_position.y), river_slw_render_vector(n));
    let direct = river_slw_direct(n, v, l, diffuse_color, specular_color, roughness);
    var color = diffuse_color * indirect + (direct.diffuse + direct.specular) * sun * shadow;

    // EvaluateWaterVolumeLighting on the copy of the scene without the river.
    let size = vec2<f32>(textureDimensions(scene_depth));
    let uv_min = 0.55 / size;
    let uv_max = (size - 0.55) / size;
    let viewport_uv = clamp(input.clip.xy / size, uv_min, uv_max);
    let ior = river_slw_ior(specular);
    let pixel = river_gather_behind(viewport_uv, size, pixel_depth, slope);
    let distortion = river_slw_uv_distortion(normalize(river_to_view(n)), river_to_view(vec3<f32>(0.0, 0.0, 1.0)),
        surface.refraction, vec2<f32>(1.0), river_view.distortion, pixel.scene_depth, pixel_depth);
    let distorted_uv = clamp(viewport_uv + distortion, uv_min, uv_max);
    let distorted = river_gather_behind(distorted_uv, size, pixel_depth, slope);
    var behind_depth = distorted.scene_depth;
    var weights = distorted.weights * distorted.refraction_mask;
    var behind_uv = distorted_uv;
    if all(distorted.weights == vec4<f32>(0.0)) {
        behind_depth = pixel.scene_depth;
        weights = pixel.weights * pixel.refraction_mask;
        behind_uv = viewport_uv;
    }
    let behind_luminance = vec3<f32>(
        dot(textureGather(0, scene_color, scene_sampler, behind_uv), weights),
        dot(textureGather(1, scene_color, scene_sampler, behind_uv), weights),
        dot(textureGather(2, scene_color, scene_sampler, behind_uv), weights));
    // SvPositionToResolvedTranslatedWorld at this pixel and the behind depth.
    let behind_z = eye.z + (input.world.z - eye.z) * (behind_depth / max(pixel_depth, 1e-4));
    let volume = river_slw_volume(coefficients, pixel_depth, behind_depth, behind_luminance, 1.0, input.world.z, behind_z,
        v, n, ior, l, sun, indirect * RIVER_PI, env_brdf, shadow, vec3<f32>(1.0), water_visibility);
    color += volume.luminance;
    return vec4<f32>(color, 1.0);
}
