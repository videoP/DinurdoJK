// Lakes & ponds material: Source engine water, a port of Valve's Water_DX90 expensive
// path from the Source SDK 2013 (water_ps2x.fxc + water_ps2x_helper.h +
// Water_vs20.fxc), blended into WaterCheap_ps2x's BLEND path (env_cubemap
// reflection faded in by distance) when underwater.w > 0, combos REFLECT=1 REFRACT=1 ABOVEWATER=1 MULTITEXTURE=0|1
// BASETEXTURE=0 BLURRY_REFRACT=0. Used with Valve's permission.
//
// Mapping onto the shared lake bindings/uniforms:
//   material    appearance.y: 0 water_canals_water2, 1 water_coast01
//   $normalmap  source_water_frames (binding 19, loaded on first use): the
//               dev/water_normal frames, then water_coast01_normal as the last
//               layer. Not loaded: the fallback normal map (binding 14) in the coast layers.
//   proxies     AnimatedTexture 30 fps / TextureScroll, from the HL2 vmts;
//               time runs at lake speed (1 = HL2 rate)
//   $refractamount / $reflectamount -> the material's values (canal 2.1/.3,
//               coast .5/.25) * transmission.y / 0.026 (the default = HL2)
//   $reflecttint / $refracttint -> 1 (HL2's vmts leave both at [1 1 1];
//               the Lake reflections slider does not apply)
//   $fogcolor / $fogend -> deep_color.rgb / transmission.w (Opaque depth)
//   _rt_WaterRefraction -> opaque scene copy; its alpha (the water height-fog
//               of the refracted texel) is rebuilt from scene depth
//   _rt_WaterReflection -> lake planar slot (SSR/sky modes unchanged)
// Source's refraction target holds only geometry below the water plane. The
// scene copy holds everything, so a displaced read that lands on above-water
// geometry keeps the undisplaced texel.

@group(1) @binding(19) var source_water_frames: texture_2d_array<f32>;

// Brush texture tiling measured from HL2 BSP texinfo (40 JKA units = 1 m):
// water_canals_water2 on d1_canals_07 repeats every 128 units, water_coast01
// on d2_coast_01/03 every 768 units.
const SOURCE_WATER_CANAL_TILE_M: f32 = 3.2;
const SOURCE_WATER_COAST_TILE_M: f32 = 19.2;

fn source_water_coast() -> bool { return lake.appearance.y > 0.5; }

// HL2 vmt $refractamount (x) and $reflectamount (y), scaled by the slider
// relative to its 0.026 default.
fn source_water_amounts() -> vec2<f32> {
    let authored = select(vec2<f32>(2.1, 0.3), vec2<f32>(0.5, 0.25), source_water_coast());
    return authored * (lake.transmission.y / 0.026);
}

// Explicit gradients: the material and load branches below then carry no
// uniformity requirement (derivatives are taken once, unconditionally).
fn source_water_coast_sample(uv: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>, loaded: bool) -> vec3<f32> {
    if loaded {
        let layer = textureNumLayers(source_water_frames) - 1u;
        return textureSampleGrad(source_water_frames, jka_lake_repeat, uv, layer, gx, gy).rgb;
    }
    return textureSampleGrad(lake_fallback_normal, jka_lake_repeat, uv, gx, gy).rgb;
}

// Returns Source's decoded tangent normal: xy along world X/Z, z up.
// `beneath`: the $bottommaterial's proxies (water_coast01_beneath scrolls its
// $bumptransform at .05 / 45 degrees; the canal beneath matches its surface).
fn source_water_normal(world: vec2<f32>, beneath: bool, ninja: vec2<f32>) -> vec3<f32> {
    let oriented = lake.wind.w > 0.5;
    let dir = lake.wind.xy;
    let local = select(world,
        vec2<f32>(dot(world, dir), dot(world, vec2<f32>(-dir.y, dir.x))), oriented);
    let lx = dpdx(local);
    let ly = dpdy(local);
    let t = camera.time * lake.deformation.w;
    var n: vec3<f32>;
    let loaded = textureDimensions(source_water_frames).x > 1u;
    if loaded && !source_water_coast() {
        // water_canals_water2: AnimatedTexture on $bumpframe at 30 fps,
        // TextureScroll .05 at 45 degrees, no $scroll1 (single sample).
        let uv = local / SOURCE_WATER_CANAL_TILE_M + vec2<f32>(0.70710678, 0.70710678) * (0.05 * t);
        let frames = max(textureNumLayers(source_water_frames), 2u) - 1u;
        let frame = u32(floor(t * 30.0)) % frames;
        n = textureSampleGrad(source_water_frames, jka_lake_repeat, uv, frame,
            lx / SOURCE_WATER_CANAL_TILE_M, ly / SOURCE_WATER_CANAL_TILE_M).rgb * 2.0 - 1.0;
    } else {
        // water_coast01: Water_vs20 MULTITEXTURE with $scroll1 [.01 .01],
        // $scroll2 [-.025 .025], TextureScroll .02 at 25 degrees; ps2x average.
        let tc = local / SOURCE_WATER_COAST_TILE_M;
        let gx = lx / SOURCE_WATER_COAST_TILE_M;
        let gy = ly / SOURCE_WATER_COAST_TILE_M;
        let f45 = vec2<f32>(tc.x + tc.y, tc.y - tc.x);
        let scroll = select(vec2<f32>(0.90630779, 0.42261826) * 0.02,
            vec2<f32>(0.70710678, 0.70710678) * 0.05, beneath);
        let n0 = source_water_coast_sample(tc + scroll * t, gx, gy, loaded);
        let n1 = source_water_coast_sample(f45 * 0.1 + vec2<f32>(0.01, 0.01) * t,
            vec2<f32>(gx.x + gx.y, gx.y - gx.x) * 0.1, vec2<f32>(gy.x + gy.y, gy.y - gy.x) * 0.1, loaded);
        let n2 = source_water_coast_sample(vec2<f32>(tc.y, tc.x) * 0.45 + vec2<f32>(-0.025, 0.025) * t,
            gx.yx * 0.45, gy.yx * 0.45, loaded);
        n = 0.33 * (n0 + n1 + n2) * 2.0 - 1.0;
    }
    if oriented {
        n = vec3<f32>(dir.x * n.x - dir.y * n.y, dir.y * n.x + dir.x * n.y, n.z);
    }
    // Shared lake relief and player wakes tilt the same tangent normal.
    let relief = jka_lake_surface(world).yz;
    let wake = lake_wake(world) + ninja;
    return vec3<f32>(n.xy - relief + wake, n.z);
}

// Source's refraction alpha: the fraction of the ray below the water times
// its depth over $fogend, i.e. the underwater path length / fog end.
fn source_water_fog(below: f32, ray: vec3<f32>) -> f32 {
    let path = below * length(ray) / max(abs(ray.y), 1e-3);
    return saturate(path / max(lake.transmission.w, 0.001));
}

// The colour mix toward the fog colour for a water path of `t` fog distances:
// the same curve the Source underwater post pass applies (source_underwater.wgsl),
// so sand just under the surface matches sand seen from above through the same water.
// DinurdoJK (not HL2, whose one scalar 1 - exp2(-4.32 t) fades every channel alike):
// per-channel absorption from the fog colour. Its brightest channel keeps HL2's
// curve (the fog distance); a darker channel is absorbed up to 5x faster, as water
// drops red first, so shallow sand turns turquoise under a teal water. A grey fog
// colour gives HL2's curve exactly.
fn source_water_fog_mix(t: f32) -> vec3<f32> {
    let c = lake.deep_color.rgb;
    let peak = max(max(c.r, c.g), max(c.b, 1e-4));
    let rate = vec3<f32>(1.0) + 4.0 * (vec3<f32>(1.0) - saturate(c / peak));
    return vec3<f32>(1.0) - exp2(-4.321928 * max(t, 0.0) * rate);
}

fn source_water_reflection(world: vec3<f32>, up: vec3<f32>, offset: vec2<f32>,
        fallback: vec3<f32>) -> vec3<f32> {
    // Planar: Source adds the normal offset directly to the projected UV.
    if lake.reflection.y > 1.5 && lake.reflection.x >= 0.0 {
        let projected = camera.view_projection * vec4<f32>(world.x, 0.0, world.z, 1.0);
        if projected.w <= 0.0 { return fallback; }
        let uv = clamp(projected.xy / projected.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5) + offset,
            vec2<f32>(0.001), vec2<f32>(0.999));
        let reflected = textureSampleLevel(jka_lake_planar, jka_lake_planar_sampler, uv,
            i32(lake.reflection.x), 0.0);
        return reflected.rgb + fallback * (1.0 - clamp(reflected.a + 1.0, 0.0, 1.0));
    }
    return jka_lake_reflection(world, up, fallback);
}

// r_lakeUnderwaterMode 6 (lake.underwater.x == 2): the $bottommaterial,
// Water_DX90 with ABOVEWATER=0, REFRACT=1, REFLECT=0 (neither beneath vmt sets
// $reflecttexture). water_canals_waterbeneath2 / water_coast01_beneath:
// $refractamount .5 / 1. The lake's fog is applied by the underwater post pass.
fn source_water_beneath(input: VertexOut, n: vec3<f32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let pixel = clamp(vec2<i32>(input.clip.xy), vec2<i32>(0), size - 1);
    // waterFogDepthValue = 1 underwater, so the scale is not reduced.
    let amount = select(0.5, 1.0, source_water_coast()) * (lake.transmission.y / 0.026);
    let uv = input.clip.xy / vec2<f32>(size);
    let shifted = clamp(vec2<i32>(clamp(uv + n.xy * amount, vec2<f32>(0.0), vec2<f32>(1.0))
        * vec2<f32>(size)), vec2<i32>(0), size - 1);
    // _rt_WaterRefraction underwater holds only the above-water view
    // (DF_CLIP_BELOW | DF_RENDER_ABOVEWATER). The scene copy also holds
    // submerged geometry, so a displaced read landing on it keeps the
    // undisplaced texel.
    var refract_pixel = pixel;
    let depth = textureLoad(scene_depth, shifted, 0);
    if depth <= 0.0 {
        refract_pixel = shifted;
    } else {
        let ndc = (vec2<f32>(shifted) + 0.5) / vec2<f32>(size)
            * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
        let hit_h = camera.inverse_view_projection * vec4<f32>(ndc, depth, 1.0);
        if hit_h.y / hit_h.w > input.world.y { refract_pixel = shifted; }
    }
    // The water fog is applied once, to this surface and the scene alike, by
    // the Source underwater post pass (a separate surface lerp left a hard
    // waterline against the sand).
    return vec4<f32>(textureLoad(scene_color, refract_pixel, 0).rgb, 1.0);
}

@fragment
fn jka_source_water_fragment(input: VertexOut) -> @location(0) vec4<f32> {
    if !jka_water_contains(input.world.xz) { discard; }
    let below = camera.position.y < input.world.y;
    let source_beneath = lake.underwater.x > 1.5 && below;
    // Fluid Ninja lake wake (player-following window): normal tilt + foam.
    let ninja = jka_lake_ninja_wake(input.world.xz);
    // Caustics LOD: surface position derivatives, taken before any branch.
    let caustic_gx = dpdx(input.world.xz);
    let caustic_gy = dpdy(input.world.xz);
    let n = source_water_normal(input.world.xz, source_beneath, ninja.xy);
    if source_beneath {
        return source_water_beneath(input, n);
    }
    let up = normalize(vec3<f32>(n.x, n.z, n.y));
    let view = normalize(camera.position - input.world);
    // Godot fog / Fluid Ninja underwater modes: the shared lake submerged view.
    if lake.underwater.x > 0.5 && below {
        return lake_underwater(input, up, view);
    }
    let size = vec2<i32>(textureDimensions(scene_depth));
    let pixel = clamp(vec2<i32>(input.clip.xy), vec2<i32>(0), size - 1);
    let behind = behind_at(pixel, input.world, input.depth.x);
    let fog_unwarped = select(
        saturate(max(behind.in_front, 0.0) / max(lake.transmission.w, 0.001)), 1.0, behind.open);
    // reflectRefractScale *= waterFogDepthValue: no distortion at the shore.
    let amounts = source_water_amounts() * fog_unwarped;
    let scale = amounts.x;
    let uv = input.clip.xy / vec2<f32>(size);
    // Refract texcoords are screen-down; reflect texcoords are screen-up.
    let refract_uv = clamp(uv + n.xy * scale, vec2<f32>(0.0), vec2<f32>(1.0));
    let shifted = clamp(vec2<i32>(refract_uv * vec2<f32>(size)), vec2<i32>(0), size - 1);
    var refract_pixel = pixel;
    var fog = fog_unwarped;
    var fog_color_mix = select(
        source_water_fog_mix(max(behind.in_front, 0.0) / max(lake.transmission.w, 0.001)),
        vec3<f32>(1.0), behind.open);
    let depth = textureLoad(scene_depth, shifted, 0);
    if depth <= 0.0 {
        refract_pixel = shifted;
        fog = 1.0;
        fog_color_mix = vec3<f32>(1.0);
    } else {
        let ndc = (vec2<f32>(shifted) + 0.5) / vec2<f32>(size)
            * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
        let hit_h = camera.inverse_view_projection * vec4<f32>(ndc, depth, 1.0);
        let hit = hit_h.xyz / hit_h.w;
        let across = hit.xz - camera.position.xz;
        let below = input.world.y - (hit.y + camera.curvature * dot(across, across));
        if below > 0.0 {
            refract_pixel = shifted;
            fog = source_water_fog(below, hit - camera.position);
            fog_color_mix = source_water_fog_mix(
                below * length(hit - camera.position) / max(abs(hit.y - camera.position.y), 1e-3)
                    / max(lake.transmission.w, 0.001));
        }
    }
    var refraction = textureLoad(scene_color, refract_pixel, 0).rgb;
    // r_lakeCaustics: Fluid Ninja caustics on the bed under the water, before
    // the depth fade so deeper water dims them.
    if lake.appearance.z > 0.5 && !behind.open {
        let bed_depth = textureLoad(scene_depth, refract_pixel, 0);
        if bed_depth > 0.0 {
            let bed_ndc = (vec2<f32>(refract_pixel) + 0.5) / vec2<f32>(size)
                * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
            let bed_h = camera.inverse_view_projection * vec4<f32>(bed_ndc, bed_depth, 1.0);
            let bed = bed_h.xyz / bed_h.w;
            if bed.y < input.world.y {
                refraction *= jka_lake_ninja_caustics(bed, input.world, caustic_gx, caustic_gy);
            }
        }
    }
    refraction = mix(refraction, lake.deep_color.rgb, fog_color_mix);

    let sky_dir = reflect(-view, up);
    let sky = env_radiance(sky_atlas, sky_sampler, sky_env,
        normalize(vec3<f32>(sky_dir.x, max(sky_dir.y, 0.0), sky_dir.z)),
        clamp(lake.appearance.x, 0.08, 1.0)) * jka_water_sky_admission(input.world.xz);
    // WaterCheap BLEND: g_CheapWaterStart/End ($cheapwaterstartdistance /
    // enddistance), here start = underwater.w metres and end = 1.5x start.
    let start = lake.underwater.w;
    let distance = length(camera.position - input.world);
    let cheap = select(0.0, saturate((distance - start) / max(start * 0.5, 1e-3)), start > 0.0);
    var reflection = sky;
    if cheap < 1.0 {
        reflection = source_water_reflection(input.world, up,
            vec2<f32>(n.x, -n.y) * amounts.y, sky);
    }

    // Fresnel against the unnormalized averaged normal, no F0, faded out
    // where the water is shallow (waterFogDepthValue < 0.1).
    let n_dot_v = saturate(dot(view, vec3<f32>(n.x, n.z, n.y)));
    let grazing = pow(1.0 - n_dot_v, 5.0);
    let fresnel = grazing * saturate((fog - 0.05) * 20.0);
    var water = mix(refraction, reflection, fresnel);
    if cheap > 0.0 {
        // WaterCheap_ps2x over the refraction: env_cubemap (the lake's sky
        // environment, baked from the map's skybox faces or the Blue Water
        // sky) with alpha saturate(fresnel + distance fade), faded at the
        // shore by the unwarped refraction alpha (REFRACTALPHA).
        let alpha = saturate(grazing + cheap) * saturate((fog_unwarped - 0.05) * 20.0);
        water = mix(water, mix(refraction, sky, alpha), cheap);
    }
    // Wake foam (Lake material BaseColor, unlit here): lit by the lake's sky radiance.
    if ninja.z > 0.0 {
        let foam_light = max(env_radiance(sky_atlas, sky_sampler, sky_env, vec3<f32>(0.0, 1.0, 0.0), 1.0)
            * jka_water_sky_admission(input.world.xz), vec3<f32>(0.05));
        water = mix(water, foam_light * 0.85, saturate(ninja.z));
    }
    if lake.offset.z > 0.5 { water = aerial_apply(water, input.world); }
    return vec4<f32>(water, 1.0);
}
