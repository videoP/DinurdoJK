// godot-water-shader's surface (`shaders/g_wave.gdshader`, fragment) under Blue Water's lighting, for `r_oceanType 5`
// (`shader_composer::SURFACE_GODOT_WATER`). Its albedo (the height-blended colour with its persistent and intersection
// foam painted in) is lit by Blue Water's sun and sky, and its refracted scene (EMISSION) passes its depth fade; the
// pair replaces Blue Water's water body. Blue Water keeps the sky and screen-space reflections, Fresnel, glints, crest
// transmission, shadows, fog and the underside. Upstream's RIM and its own specular are Blue Water's here.

// g_wave's foam colours (linear). Its `color` (FoamScene: sRGB 0, 55, 65) and crest colour (linear 0, 0.08, 0.02) are
// `jka_hybrid.godot_color` and `jka_hybrid.godot_crest`.
const JKA_GODOT_FOAM_COLOR: vec3<f32> = vec3<f32>(0.6, 1.0, 0.9);
const JKA_GODOT_STRIP_COLOR: vec3<f32> = vec3<f32>(0.4, 0.7, 0.6);
// FoamScene's `absorption_strength` (1.25) times `color_absorption` (1.3, 0.4, 0.6), per metre.
const JKA_GODOT_ABSORPTION: vec3<f32> = vec3<f32>(1.625, 0.5, 0.75);
// The foam viewports' `capture_coverage` (upstream metres): FoamSubViewport and FoamSubViewportFar.
const JKA_GODOT_NEAR_COVERAGE: f32 = 15.0;
const JKA_GODOT_FAR_COVERAGE: f32 = 30.0;
const JKA_GODOT_INV_PI: f32 = 0.3183098861837907;

fn jka_godot_mixed_foam(foam_old: f32, foam_fresh: f32, foam_in: vec2<f32>) -> vec2<f32> {
    var foam = foam_in;
    var mixed_foam_j = mix(foam_old, foam_fresh, foam.x);
    mixed_foam_j = smoothstep(0.4, 1.0, mixed_foam_j);
    foam.x = clamp(foam.x * mixed_foam_j, 0.0, 1.0);
    foam.y *= mix(foam_old, foam_fresh, foam.y);
    foam.y = smoothstep(0.4, 1.0, foam.y);
    return foam;
}

// Footprint anisotropy the per-pixel waves accept before averaging (Blue Water's own wave sample takes 16 with
// anisotropic texture reads; point-sampled analytic waves alias past a few texels along the long side).
const JKA_GODOT_ANISOTROPY: f32 = 4.0;

struct JkaGodotPixel {
    // Height slope of the displaced surface (d height / d grid xz), the waves' horizontal Jacobian, and the slope
    // variance the pixel averaged away.
    slope: vec2<f32>,
    jacobian: f32,
    variance: f32,
}

// The waves at grid point `grid` (m) for this pixel, analytically (upstream takes finite differences per vertex of its
// 4 cm plane): each wave faded by the pixel's footprint. The height noise displaces the vertices but adds no slope here:
// its five scattered reads cost 1.1 ms of a 3.7 ms surface at costa (r_oceanCostProbe A/B, 2026-10-21) for 5 cm
// bumps. Takes screen-space derivatives: call in uniform control flow.
fn jka_godot_pixel(grid: vec2<f32>) -> JkaGodotPixel {
    let scale = jka_hybrid.godot.y;
    let time = jka_godot_time(jka_hybrid.godot.z, scale);
    let pos = grid / scale;
    let dx = dpdx(pos);
    let dy = dpdy(pos);
    let across = length(dx);
    let down = length(dy);
    let footprint = max(min(across, down), max(across, down) / JKA_GODOT_ANISOTROPY);
    let surface = jka_godot_surface(pos, time, footprint);
    let normal = surface.normal;
    return JkaGodotPixel(-normal.xz / max(normal.y, 0.05), normal.y, surface.variance);
}

// g_wave's foam at grid point `grid` (m): the near map fading into the far one, and both out at the far map's edge. A
// disabled map reads as upstream's 1x1 black replacement. Past the maps (`r_gerstnerFoam` bit 4), foam_calc's
// compression foam from this pixel's own `jacobian`, without the history's persistence.
fn jka_godot_foam(grid: vec2<f32>, jacobian: f32) -> vec2<f32> {
    let scale = jka_hybrid.godot.y;
    let flags = u32(jka_hybrid.godot.w + 0.5);
    let near_on = (flags & 1u) != 0u;
    let far_on = (flags & 2u) != 0u;
    let distant_on = (flags & 4u) != 0u;
    let near_uv = (grid - jka_hybrid.godot_foam.xy) / (JKA_GODOT_NEAR_COVERAGE * scale) + 0.5;
    let far_uv = (grid - jka_hybrid.godot_foam.zw) / (JKA_GODOT_FAR_COVERAGE * scale) + 0.5;
    let foam_near = textureSampleLevel(jka_godot_foam_near, wake_sampler, near_uv, 0.0).rg * f32(near_on);
    let foam_far = textureSampleLevel(jka_godot_foam_far, wake_sampler, far_uv, 0.0).rg * f32(far_on);
    let world_uv = grid / scale / 15.0;
    let foam_fresh = textureSample(jka_godot_patterns, jka_godot_repeat, world_uv * 2.0).r;
    let foam_old = textureSample(jka_godot_patterns, jka_godot_repeat, world_uv * 24.0).r;
    let foam_near_mixed = jka_godot_mixed_foam(foam_old, foam_fresh, foam_near);
    let foam_far_mixed = jka_godot_mixed_foam(foam_old, foam_fresh, foam_far);
    let camera_dist = distance(grid, camera.position.xz) / scale;
    let near_radius = JKA_GODOT_NEAR_COVERAGE * 0.5;
    let far_radius = JKA_GODOT_FAR_COVERAGE * 0.5;
    let fade_near = select(1.0, smoothstep(near_radius * 0.7, near_radius, camera_dist), near_on);
    let fade_far = smoothstep(far_radius * 0.9, far_radius, camera_dist);
    // Without the far map the near one fades to its black replacement on its own.
    let persistent = select(foam_near_mixed, mix(foam_near_mixed, foam_far_mixed, fade_near), far_on);
    // Where the persistent maps end.
    let beyond = select(select(1.0, fade_near, near_on), fade_far, far_on);
    let compression = 1.0 - smoothstep(0.0, 0.7, jacobian);
    let noise_v = smoothstep(0.2, 0.7, jka_godot_noise(world_uv)) * 8.0;
    let distant = jka_godot_mixed_foam(foam_old, foam_fresh, vec2<f32>(clamp(compression * noise_v, 0.0, 1.0), 0.0))
        * f32(distant_on);
    return mix(persistent, distant, beyond);
}

// The water body at grid point `grid`: `scene` is the (refracted) opaque scene and `through` the water it crosses (m,
// negative for none); `contact_depth` is upstream's `through_water` for the intersection strip, the view-depth step from
// the surface to the scene behind it (m, negative where nothing stands behind). `sun_irradiance`/`sky_irradiance` are
// Blue Water's.
fn jka_godot_body(grid: vec2<f32>, world: vec3<f32>, scene: vec3<f32>, through: f32, contact_depth: f32, up: vec3<f32>,
    sun: vec3<f32>, sun_irradiance: vec3<f32>, sky_irradiance: vec3<f32>, lit: f32, jacobian: f32) -> vec3<f32> {
    let scale = jka_hybrid.godot.y;
    let time = jka_godot_time(jka_hybrid.godot.z, scale);
    let h_blend = smoothstep(-0.7, 1.0, world.y / scale);
    var albedo = mix(jka_hybrid.godot_color.rgb * 1.2, jka_hybrid.godot_crest.rgb, h_blend);
    let foam = jka_godot_foam(grid, jacobian);
    albedo = mix(albedo, JKA_GODOT_FOAM_COLOR, foam.x);
    albedo = mix(albedo, JKA_GODOT_FOAM_COLOR, foam.y);
    // Upstream reads its i_noise at the 10 m plane's UV * 20: two repeats a metre.
    let i_noise = textureSample(jka_godot_patterns, jka_godot_repeat, grid / scale * 2.0 + time * 0.03).b;
    // A contact line, not a wave feature: upstream's metres at any wave scale.
    let i_width = 0.05 + sin(time * 0.5) * 0.02 + i_noise * 0.04;
    let fade_in = smoothstep(0.0, 0.1 * i_width, contact_depth);
    let fade_out = 1.0 - smoothstep(0.8 * i_width, i_width, contact_depth);
    let strip = select(0.0, fade_in * fade_out, contact_depth > 0.0 && contact_depth < i_width);
    albedo = mix(albedo, JKA_GODOT_STRIP_COLOR, strip);
    let diffuse = albedo * JKA_GODOT_INV_PI * (sun_irradiance * (lit * max(dot(up, sun), 0.0)) + sky_irradiance);
    return scene * exp(-JKA_GODOT_ABSORPTION * max(through, 0.0)) + diffuse;
}
