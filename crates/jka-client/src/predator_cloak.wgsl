// Source reference: ektogamat/predator-material, src/PredatorMaterial/
// PredatorMaterial.js + src/gridSettings.js + src/glassSettings.js.
// This file translates the authored procedural material *signals* to WGSL.
// Three.js MeshPhysicalNodeMaterial BRDF/IBL/transmission is NOT part of this
// codebase, so the scene-colour compositing below is a renderer adaptation,
// NOT a numerically identical port of Three.js physical shading.
// Keep reference defaults visible here so changes can be compared upstream.
const PREDATOR_UV_SCALE: f32 = 3.0;
const PREDATOR_FADE_POWER: f32 = 3.5;
const PREDATOR_SIZE_MAX: f32 = 20.5;
const PREDATOR_CELL_OFFSET: f32 = 0.25;
const PREDATOR_RING_DENSITY: f32 = 14.0;
const PREDATOR_RING_WIDTH: f32 = 0.12;
const PREDATOR_RING_SPEED: f32 = 0.35;
const PREDATOR_SHIMMER_SPEED: f32 = 2.0;
const PREDATOR_SHIMMER_SCALE: f32 = 15.0;
const PREDATOR_SHIMMER_NORMAL_SCALE: f32 = 10.5;
const PREDATOR_SHIMMER_MIN: f32 = 0.6;
const PREDATOR_METAL_IN_CELL: f32 = 0.003;
const PREDATOR_IOR: f32 = 1.16;
const PREDATOR_THICKNESS: f32 = 1.6;
const PREDATOR_DISPERSION: f32 = 4.1;
const PREDATOR_ROUGHNESS: f32 = 0.3;
const PREDATOR_METALNESS: f32 = 0.42;
const PREDATOR_ENV_INTENSITY: f32 = 1.8;
const PREDATOR_FRESNEL_INTENSITY: f32 = 0.2;
const PREDATOR_CLEARCOAT: f32 = 0.07;
const PREDATOR_CLEARCOAT_ROUGHNESS: f32 = 0.14;
const PREDATOR_IRIDESCENCE: f32 = 0.21;
const PREDATOR_IRIDESCENCE_IOR: f32 = 1.36;
const PREDATOR_IRIDESCENCE_THICKNESS_MIN: f32 = 0.0;
const PREDATOR_IRIDESCENCE_THICKNESS_MAX: f32 = 1000.0;
// THREE.Color('#e6e6e6') and '#f2f7ff' in linear working colour space.
const PREDATOR_CELL_COLOR: vec3<f32> = vec3<f32>(0.79129794, 0.79129794, 0.79129794);
const PREDATOR_FRESNEL_COLOR: vec3<f32> = vec3<f32>(0.8879231, 0.9301109, 1.0);

// TSL's mod(x, y) uses a floor-based remainder. WGSL % on floats does not
// necessarily share GLSL mod's signed input semantics: implement explicitly.
fn predator_mod(value: vec2<f32>, period: f32) -> vec2<f32> {
    return value - period * floor(value / period);
}
fn predator_uv_mask(st: vec2<f32>, grid: vec2<f32>, progress: f32) -> f32 {
    let fade_timer = progress * 1.3;
    let pos_i = st * grid * PREDATOR_UV_SCALE;
    let final_pos = predator_mod(pos_i, 2.0) - vec2<f32>(1.0, 2.0);
    let pos_i2 = floor(pos_i * 0.5) / grid;
    let size = abs(clamp(pow(fade_timer + pos_i2.y, PREDATOR_FADE_POWER),
        0.0, PREDATOR_SIZE_MAX) * progress);
    return select(0.0, 1.0, abs(final_pos.x + PREDATOR_CELL_OFFSET) + abs(final_pos.y) < size);
}
fn predator_ring_mask(position: vec3<f32>, progress: f32, time: f32) -> f32 {
    // Source default center (0,0,0), axis scale (1,1,1).
    let dist = length(position);
    let phase = dist * PREDATOR_RING_DENSITY - time * PREDATOR_RING_SPEED;
    let rings = abs(fract(phase) - 0.5) * 2.0;
    let lines = 1.0 - smoothstep(0.0, PREDATOR_RING_WIDTH, rings);
    let reveal_radius = PREDATOR_SIZE_MAX * clamp(pow(progress * 1.3, 1.0 / PREDATOR_FADE_POWER), 0.0, 1.0);
    return lines * select(0.0, 1.0, dist < reveal_radius);
}
fn predator_mask(st: vec2<f32>, grid: vec2<f32>, local: vec3<f32>, world: vec3<f32>,
                 view: vec3<f32>, projection: f32, progress: f32, time: f32) -> f32 {
    if projection < 0.5 { return predator_uv_mask(st, grid, progress); }
    if projection < 1.5 { return predator_ring_mask(local, progress, time); }
    if projection < 2.5 { return predator_ring_mask(world, progress, time); }
    return predator_ring_mask(view, progress, time);
}

// Three.js positionView is world position transformed by the camera's
// view matrix. Projecting into a camera-aligned orthonormal basis gives the
// exact translation/rotation (perspective scaling is NOT applied).
// Matrix rows 0/1 of P * V are right/up times the positive projection scales.
fn predator_position_view(world: vec3<f32>) -> vec3<f32> {
    let relative = world - camera.camera_pos_time.xyz;
    let right = normalize(vec3<f32>(camera.view_proj[0].x,
        camera.view_proj[1].x, camera.view_proj[2].x));
    let up = normalize(vec3<f32>(camera.view_proj[0].y,
        camera.view_proj[1].y, camera.view_proj[2].y));
    let forward = normalize(camera.camera_forward.xyz);
    return vec3<f32>(dot(relative, right), dot(relative, up), -dot(relative, forward));
}

// No invented mask styles: values 0..3 select the four branches of the
// source author's buildGridMask() TSL graph. The reference ships with 0.
// Projection comes from the draw's blue channel only for Predator materials.
// All other grid/ring parameters retain the original gridSettings.js values.
fn predator_shade(
    fragment: vec4<f32>, st: vec2<f32>, normal: vec3<f32>, world: vec3<f32>,
    local: vec3<f32>, view: vec3<f32>, grid: vec2<f32>, progress: f32,
    projection: f32, time: f32, original_color: vec3<f32>
) -> vec4<f32> {
    let p = clamp(progress, 0.0, 1.0);
    let grid_mask = predator_mask(st, grid, local, world, view, projection, p, time);
    let material_color = mix(original_color, PREDATOR_CELL_COLOR, grid_mask);
    let n = normalize(normal);
    let v = normalize(camera.camera_pos_time.xyz - world);
    let nv = clamp(dot(n, v), 0.0, 1.0);
    let shimmer = clamp(pow(sin(p * PREDATOR_SHIMMER_SCALE + time * PREDATOR_SHIMMER_SPEED
        + n.y * PREDATOR_SHIMMER_NORMAL_SCALE), 2.0), PREDATOR_SHIMMER_MIN, 1.0);
    let roughness = mix(PREDATOR_ROUGHNESS, shimmer, grid_mask);
    let metalness = mix(PREDATOR_METALNESS, PREDATOR_METAL_IN_CELL, grid_mask);
    let glass = smoothstep(0.0, 0.08, p);
    let fresnel_mask = smoothstep(0.4, 0.85, p);
    let emissive = PREDATOR_FRESNEL_COLOR * pow(1.0 - nv, 4.0)
        * fresnel_mask * PREDATOR_FRESNEL_INTENSITY;

    // Integration adapter: the actual upstream `MeshPhysicalNodeMaterial`
    // samples its transmission render target and environment using Three's
    // GGX, multiple scattering, iridescence, clearcoat and IBL models.
    // JKA presently supplies a screen-colour capture instead. Do not label
    // this optical response a faithful replacement for those Three.js nodes.
    let dimensions = vec2<f32>(textureDimensions(glass_scene));
    let screen_uv = glass_clamp_uv(fragment.xy / dimensions);
    let refracted_direction = refract(-v, n, 1.0 / PREDATOR_IOR);
    let clip = camera.view_proj * vec4<f32>(world + refracted_direction * PREDATOR_THICKNESS, 1.0);
    let projected_uv = vec2<f32>(clip.x / max(abs(clip.w), 0.00001) * 0.5 + 0.5,
        0.5 - clip.y / max(abs(clip.w), 0.00001) * 0.5);
    let fraction = glass_clamp_uv(projected_uv);
    // Explicit channel-wise dispersion and roughness LOD are a stand-in for
    // the Three.js transmissive model; glass disables neither normal sampling
    // nor the source's procedural mask and fresnel emissive layer.
    let channel_step = (fraction - screen_uv) * PREDATOR_DISPERSION * 0.015 * glass;
    let lod = clamp(roughness * 3.0, 0.0, f32(max(textureNumLevels(glass_scene), 1u) - 1u));
    let transmission_rgb = vec3<f32>(
        textureSampleLevel(glass_scene, glass_sampler, glass_clamp_uv(fraction + channel_step), lod).r,
        textureSampleLevel(glass_scene, glass_sampler, fraction, lod).g,
        textureSampleLevel(glass_scene, glass_sampler, glass_clamp_uv(fraction - channel_step), lod).b
    );
    let f0 = pow((PREDATOR_IOR - 1.0) / (PREDATOR_IOR + 1.0), 2.0);
    let spec_f0 = mix(vec3<f32>(f0), material_color, metalness);
    let fresnel = spec_f0 + (vec3<f32>(1.0) - spec_f0) * pow(1.0 - nv, 5.0);
    let clearcoat_f0 = pow((1.5 - 1.0) / (1.5 + 1.0), 2.0);
    let clearcoat_fresnel = clearcoat_f0 + (1.0 - clearcoat_f0) * pow(1.0 - nv, 5.0);
    let coat = PREDATOR_CLEARCOAT * clearcoat_fresnel;
    let color = mix(material_color, transmission_rgb * (vec3<f32>(1.0) - fresnel), glass)
        + (fresnel * roughness * PREDATOR_ENV_INTENSITY * glass)
        + vec3<f32>(coat * PREDATOR_CLEARCOAT_ROUGHNESS) + emissive;
    return vec4<f32>(color, 1.0);
}
