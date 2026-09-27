// Optimized visible-blade render path for GodotGrass.
// Root-level noise/wind is prepared once per blade by grass_prepare.wgsl.

struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

// Pipeline-specialized: false compiles the self-applied Legacy fog out entirely.
// Rebuilt only when FogSystem::legacy_self_fog toggles, never checked per frame.
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
    // x: 0 off, 1 authored global EXP2, 2 manual; y: strength scale.
    legacy_fog_params: vec4<f32>,
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
@group(1) @binding(2) var noise_sampler: sampler;
@group(1) @binding(3) var<uniform> grass: GrassGlobals;
@group(1) @binding(4) var blade_detail: texture_2d<f32>;
@group(2) @binding(0) var shadow_texture: texture_depth_2d_array;
@group(2) @binding(1) var shadow_sampler: sampler_comparison;
@group(2) @binding(2) var<uniform> shadow_settings: ShadowSettings;
@group(2) @binding(7) var sky_admission_texture: texture_depth_2d_array;

struct PreparedWords { words: array<u32>, };
@group(3) @binding(0) var<storage, read> prepared_instances: PreparedWords;

const PI: f32 = 3.14159265358979323846;
const PREPARED_WORDS: u32 = 11u;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @builtin(instance_index) instance_index: u32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) camera_distance_m: f32,
    @location(4) blade_random: f32,
    // RGB is the blade-root baked light; A is the per-root weather wetness
    // computed once in grass_prepare.wgsl. A vec3 already consumes one varying
    // location, so carrying alpha here adds no additional location.
    @location(5) baked_light: vec4<f32>,
    @location(6) ground_tint: vec4<f32>,
};

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
    if (any(ndc.xy < vec2<f32>(-1.0)) || ndc.z < 0.0 || any(ndc > vec3<f32>(1.0))) {
        return 0.0;
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
    if (cascade < 2i) {
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

struct LoadedPrepared {
    root_world: vec3<f32>,
    authored_height_scale: f32,
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
    baked_light: vec4<f32>,
    ground_tint: vec4<f32>,
};

fn load_prepared(index: u32) -> LoadedPrepared {
    let base = index * PREPARED_WORDS;
    let camera_crush = unpack2x16float(prepared_instances.words[base + 4u]);
    let hw = unpack2x16float(prepared_instances.words[base + 5u]);
    let turn = unpack2x16float(prepared_instances.words[base + 6u]);
    let bend0 = unpack2x16float(prepared_instances.words[base + 7u]);
    let bend1 = unpack2x16float(prepared_instances.words[base + 8u]);
    var result: LoadedPrepared;
    result.root_world = bitcast<vec3<f32>>(vec3<u32>(
        prepared_instances.words[base + 0u],
        prepared_instances.words[base + 1u],
        prepared_instances.words[base + 2u],
    ));
    result.authored_height_scale = bitcast<f32>(prepared_instances.words[base + 3u]);
    result.camera_distance_m = camera_crush.x;
    result.crushed_factor = camera_crush.y;
    result.height_offset = hw.x;
    result.width_scale = hw.y;
    result.sin_turn = turn.x;
    result.cos_turn = turn.y;
    result.bend_base_coeff = bend0.x;
    result.turbulence_base = bend0.y;
    result.turbulence_tip = bend1.x;
    result.blade_random = bend1.y;
    result.baked_light = unpack4x8unorm(prepared_instances.words[base + 9u]);
    result.ground_tint = unpack4x8unorm(prepared_instances.words[base + 10u]);
    return result;
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let instance = load_prepared(input.instance_index);
    let world_scale = grass.params.x;
    let height_factor = 1.0 - input.uv.y;

    var vertex_m = input.position * instance.authored_height_scale;
    vertex_m.x *= instance.width_scale;
    vertex_m.y *= instance.height_offset * 1.20;
    let vertex_model = vertex_m;

    // Root-level wind/clump inputs were evaluated once in the compute pass. The
    // original per-vertex turbulence sample shifts smoothly by h^2, so interpolate
    // the two endpoint samples by the same h^2 term here.
    let h2 = height_factor * height_factor;
    let turbulence = mix(instance.turbulence_base, instance.turbulence_tip, h2);
    let uncrushed_bend = instance.bend_base_coeff * height_factor + turbulence;
    // Source GodotGrass player interaction, tuned for a more obvious footstep:
    // retain the same proximity blend but increase the crush bend by 40%
    // (0.45*PI -> 0.63*PI). Wind is already suppressed in the compute pass.
    let bend_angle = mix(0.63 * PI, uncrushed_bend, instance.crushed_factor);
    let sin_bend = sin(bend_angle);
    let cos_bend = cos(bend_angle);

    let bent_vertex = vec3<f32>(
        vertex_m.x,
        cos_bend * vertex_m.y - sin_bend * vertex_m.z,
        sin_bend * vertex_m.y + cos_bend * vertex_m.z,
    );
    vertex_m = vec3<f32>(
        instance.cos_turn * bent_vertex.x + instance.sin_turn * bent_vertex.z,
        bent_vertex.y,
        -instance.sin_turn * bent_vertex.x + instance.cos_turn * bent_vertex.z,
    );
    let bent_normal = vec3<f32>(0.0, -sin_bend, cos_bend);
    let blade_normal = vec3<f32>(
        instance.sin_turn * bent_normal.z,
        bent_normal.y,
        instance.cos_turn * bent_normal.z,
    );

    let root_m3 = instance.root_world * world_scale;
    let camera_m = camera.camera_pos_time.xyz * world_scale;
    let camera_offset_m = root_m3 - camera_m;
    let normal_world = normalize(blade_normal);
    let dot_nv = dot(normal_world, normalize(camera_offset_m));
    let sign_vertex = instance.cos_turn * vertex_model.x + instance.sin_turn * vertex_model.z;
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

    let world_position = instance.root_world + vertex_m / world_scale;
    var out: VertexOutput;
    out.position = camera.view_proj * vec4<f32>(world_position, 1.0);
    out.uv = input.uv;
    out.world_position = world_position;
    out.normal = blade_normal;
    out.camera_distance_m = instance.camera_distance_m;
    out.blade_random = instance.blade_random;
    out.baked_light = instance.baked_light;
    out.ground_tint = instance.ground_tint;
    return out;
}

// Legacy fog: procedural grass is not a BSP material stage, so it fogs itself
// with the same curves as bsp.wgsl legacy_fog_color_amount (see
// FogSystem::legacy_self_fog). Legacy 1 authored global fog is instead composited
// in post over the grass using the ground depth behind it.
fn apply_grass_legacy_fog(rgb: vec3<f32>, world_position: vec3<f32>) -> vec3<f32> {
    if (!ENABLE_LEGACY_FOG) {
        return rgb;
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

    let wetness = clamp(input.baked_light.a, 0.0, 1.0);
    if (wetness > 0.001) {
        // Make the wet state read mainly as richer/darker grass, not as a broad
        // pale overlay. This is arithmetic-only: no extra texture lookup.
        let luma = dot(albedo, vec3<f32>(0.2126, 0.7152, 0.0722));
        let wet_color = max(mix(vec3<f32>(luma), albedo, 1.14), vec3<f32>(0.0)) * 0.74;
        albedo = mix(albedo, wet_color, wetness);
    }

    let edge_distance = min(input.uv.x, 1.0 - input.uv.x);
    let edge_mask = 1.0 - smoothstep(0.008, 0.085, edge_distance);

    // GodotGrass light(): custom diffuse that never reaches zero, plus back-light SSS.
    let toward_sun = normalize(grass.sun_direction_strength.xyz);
    let view = normalize(camera.camera_pos_time.xyz - input.world_position);
    let diffuse_factor = pow(4.0, dot(normal, toward_sun)) / 4.0;
    let sss_factor = max(-dot(view, toward_sun), 0.0) * 0.5;

    // Reuse the world's existing cascaded sun map. The first cascade also receives
    // a very cheap near-only one-triangle grass proxy; farther cascades stay world-only.
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
    let baked = clamp(input.baked_light.rgb, vec3<f32>(0.0), vec3<f32>(1.5));
    let baked_luma = dot(baked, vec3<f32>(0.2126, 0.7152, 0.0722));
    let baked_tint = select(vec3<f32>(1.0), baked / max(baked_luma, 0.06), baked_luma > 0.01);
    let ambient_strength = mix(0.08, 0.52, clamp(baked_luma, 0.0, 1.0));
    let ambient = baked_tint * ambient_strength;

    var lit = albedo * (ambient + direct) + edge_transmission;
    if (wetness > 0.001) {
        // Tiny blades do not justify the world's reflection-probe cubemap fetch.
        // Recreate the visually important wet coat with the already available sun,
        // view, normal and blade-detail data: zero additional texture samples.
        let half_sum = toward_sun + view;
        let half_vector = half_sum * inverseSqrt(max(dot(half_sum, half_sum), 1.0e-6));
        let gloss = pow(wetness, 1.20);
        // Narrower but stronger glints make wet blades visible in motion without
        // whitening whole blade faces. The broad Fresnel term is intentionally tiny.
        let specular = pow(max(abs(dot(normal, half_vector)), 0.0), mix(70.0, 170.0, gloss));
        let fresnel = pow(1.0 - abs(dot(normal, view)), 5.0);
        let micro_variation = mix(0.72, 1.0, detail.g);
        let wet_sheen = grass.sun_color.rgb
            * grass.sun_direction_strength.w
            * sun_attenuation
            * gloss
            * (specular * 0.52 * micro_variation + fresnel * 0.010);
        lit += wet_sheen;
    }
    return vec4<f32>(apply_grass_legacy_fog(lit, input.world_position), 1.0);
}

@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
