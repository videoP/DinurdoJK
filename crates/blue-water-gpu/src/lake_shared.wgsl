// Shared Source-lake helpers: the fallback normal map, the player-wake ripple
// atlas and the submerged view (Godot fog / Fluid Ninja underwater modes).
// The wake atlas and fallback normal map originate from Cheap Interactive
// Water, Mirko Kunze (2024), MIT.
@group(1) @binding(14) var lake_fallback_normal: texture_2d<f32>;
@group(1) @binding(15) var lake_wake_atlas: texture_2d_array<f32>;
@group(1) @binding(17) var lake_submerged_scene: texture_2d<f32>;
@group(1) @binding(18) var lake_submerged_depth: texture_2d<f32>;
// Fluid Ninja lake wake (jka-ninja lake_api.rs): a player-following window of the
// dense-water solver. window: centre (JKA x/y cm), 1 / extent (cm), enabled.
struct JkaNinjaLakeWake { window: vec4<f32>, params: vec4<f32> }
@group(1) @binding(23) var jka_ninja_lake_vd: texture_2d<f32>;
@group(1) @binding(24) var jka_ninja_lake_pd: texture_2d<f32>;
@group(1) @binding(25) var jka_ninja_lake_flow: texture_2d<f32>;
@group(1) @binding(26) var<uniform> jka_ninja_lake: JkaNinjaLakeWake;
@group(1) @binding(27) var jka_ninja_caustics: texture_3d<f32>;
// r_lakeCaustics (lake.appearance.z): Fluid Ninja caustics on the lakebed seen
// through the water. `bed` is the refracted bed point (native metres), `gx/gy`
// the screen derivatives of the surface position (m). Returns the colour scale
// for the bed (1 = none). The lake wake window's flow/divergence drive it there.
fn jka_lake_ninja_caustics(bed: vec3<f32>, surface: vec3<f32>, gx: vec2<f32>, gy: vec2<f32>) -> vec3<f32> {
    let bed_cm = vec2<f32>(bed.x, -bed.z) * 100.0;
    let uv = (bed_cm - jka_ninja_lake.window.xy) * jka_ninja_lake.window.z + vec2<f32>(0.5);
    let sim_on = jka_ninja_lake.window.w > 0.5 && all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0));
    return ninja_lake_caustics(jka_ninja_caustics, jka_lake_repeat, jka_ninja_lake_vd, jka_ninja_lake_pd,
        mask_sampler, uv, sim_on, bed_cm, distance(bed, surface) * 100.0,
        distance(camera.position, surface) * 100.0, camera.time, gx * 100.0, gy * 100.0);
}

// (tangent-normal tilt along world X, along world Z, foam coverage, 0); zero outside the window.
@diagnostic(off, derivative_uniformity)
fn jka_lake_ninja_wake(world: vec2<f32>) -> vec4<f32> {
    if jka_ninja_lake.window.w < 0.5 { return vec4<f32>(0.0); }
    // Native metres (x, z) -> JKA cm (x, y = -z): 1 m = 40 units = 100 cm.
    let cm = vec2<f32>(world.x, -world.y) * 100.0;
    let uv = (cm - jka_ninja_lake.window.xy) * jka_ninja_lake.window.z + vec2<f32>(0.5);
    if any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) { return vec4<f32>(0.0); }
    let w = ninja_lake_wake(jka_ninja_lake_vd, jka_ninja_lake_pd, jka_ninja_lake_flow,
        mask_sampler, jka_lake_repeat, uv, cm, camera.time);
    // Fade the window edge so the field never shows its square.
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    let fade = smoothstep(0.0, 0.1, edge);
    return vec4<f32>(w.x, -w.y, w.z, 0.0) * fade;
}
// Player wake slope (world XZ) from the bounded ripple atlas; zero outside it.
fn lake_wake(world: vec2<f32>) -> vec2<f32> {
    if lake.ripples.w < 0.0 { return vec2<f32>(0.0); }
    let ripple_uv = (world-lake.ripples.xy)*lake.ripples.z;
    if any(ripple_uv <= vec2<f32>(0.0)) || any(ripple_uv >= vec2<f32>(1.0)) { return vec2<f32>(0.0); }
    let edge = min(min(ripple_uv.x,1.0-ripple_uv.x),min(ripple_uv.y,1.0-ripple_uv.y));
    return textureSampleLevel(lake_wake_atlas,mask_sampler,ripple_uv,i32(lake.ripples.w),0.0).rg
        * smoothstep(0.0,0.08,edge);
}

// Hybrid's underside optical response, applied to the lake's SAME animated
// geometry, scrolling ripple normals and player-wake atlas. Its fog capture
// already attenuated the transmitted scene; only reflected interface light
// needs the extra interface-to-eye absorption below.
fn lake_underwater(input: VertexOut, up: vec3<f32>, view: vec3<f32>) -> vec4<f32> {
    let normal = -up;
    let facing = max(dot(normal, view), 0.00002);
    var fresnel = 0.02 + 0.98 * pow(1.0 - facing, 5.0);
    // Water -> air: the animated normals deform Snell's window too.
    if 1.333 * 1.333 * (1.0 - facing * facing) > 1.0 { fresnel = 1.0; }
    let size = vec2<i32>(textureDimensions(lake_submerged_scene));
    let uv = input.clip.xy / vec2<f32>(size);
    let pixel = clamp(vec2<i32>(input.clip.xy), vec2<i32>(0), size - 1);
    let receiver_depth = textureLoad(lake_submerged_depth, pixel, 0).r;
    let units = max(lake.underwater.z, 0.001);
    let thickness = max(receiver_depth / units - input.depth.x, 0.0);
    let edge = smoothstep(0.0, 0.06, min(min(uv.x, uv.y), min(1.0-uv.x, 1.0-uv.y)));
    let tilt = normal - vec3<f32>(0.0, -1.0, 0.0);
    let screen_normal = (camera.view_projection * vec4<f32>(tilt, 0.0)).xy * vec2<f32>(1.0, -1.0);
    let offset = clamp(screen_normal, vec2<f32>(-1.0), vec2<f32>(1.0))
        * lake.transmission.y * edge * min(thickness / 1.6, 1.0);
    let shifted = clamp(vec2<i32>((uv + offset) * vec2<f32>(size)), vec2<i32>(0), size - 1);
    let shifted_depth = textureLoad(lake_submerged_depth, shifted, 0).r;
    let ndc = (vec2<f32>(shifted) + 0.5) / vec2<f32>(size)
        * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    let near_h = camera.inverse_view_projection * vec4<f32>(ndc, 1.0, 1.0);
    let ray = normalize(near_h.xyz / near_h.w - camera.position);
    let axis = max(-(camera.view * vec4<f32>(ray, 0.0)).z, 0.001);
    let hit = camera.position + ray * (shifted_depth / units / axis);
    // The paired point read avoids smearing foreground color into refraction.
    let valid = shifted_depth >= 999999.0 || hit.y > input.world.y;
    let transmitted = textureLoad(lake_submerged_scene, select(pixel, shifted, valid), 0).rgb;
    let direction = reflect(-view, normal);
    // Under a BSP roof the global atmosphere (including its solar halo) is
    // not a valid fallback for reflections seen from below the surface.
    let reflection = env_radiance(sky_atlas, sky_sampler, sky_env, direction,
        clamp(lake.appearance.x, 0.08, 1.0)) * jka_water_sky_admission(input.world.xz);
    // Same spectral ratios, exponential falloff and depth darkening as Hybrid.
    let attenuation = jka_underwater_spectral_transmission(
        length(input.world-camera.position), max(lake.underwater_fog.w, 0.025));
    let ambient = lake.underwater_fog.rgb * jka_underwater_spectral_transmission(
        max(input.world.y-camera.position.y, 0.0), max(lake.underwater.y, 0.025));
    return vec4<f32>(transmitted * (1.0-fresnel)
        + (reflection * attenuation + ambient * (1.0-attenuation)) * fresnel, 1.0);
}
