// Host bridge: the private Ocean2 material owns all surface attributes. The
// existing UE SLW port owns water optics. Native mesh/camera/admission are host
// inputs, not part of the FNL material graph.
struct NinjaOceanHost {
    domain: vec4<f32>, // centre in native metres xz, full extent m, absolute plane m
    defaults: vec4<f32>, // three class defaults, FFT enabled
};
@group(4) @binding(0) var<uniform> ninja_host: NinjaOceanHost;
@group(4) @binding(1) var ninja_vd: texture_2d<f32>;
@group(4) @binding(2) var ninja_pd: texture_2d<f32>;
@group(4) @binding(3) var ninja_flow: texture_2d<f32>;
@group(4) @binding(4) var ninja_rough: texture_2d<f32>;
@group(4) @binding(5) var ninja_tile1: texture_2d<f32>;
@group(4) @binding(6) var ninja_tile2: texture_2d<f32>;
@group(4) @binding(7) var ninja_clamp: sampler;
@group(4) @binding(8) var ninja_repeat: sampler;
@group(4) @binding(9) var ninja_gf: texture_2d<f32>;
fn ninja_uv(xz: vec2<f32>) -> vec2<f32> {
    let uv = (xz - ninja_host.domain.xy) / ninja_host.domain.z;
    return vec2<f32>(uv.x, -uv.y) + 0.5;
}
fn ninja_domain_share(uv: vec2<f32>) -> f32 {
    // Host finite-domain handoff, not an authored FNL material parameter.
    return 1.0 - smoothstep(0.45, 0.5, max(abs(uv.x - 0.5), abs(uv.y - 0.5)));
}
fn ninja_cm(p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(p.x, -p.z, p.y + ninja_host.domain.w) * 100.0;
}
fn ninja_displacement(grid: vec2<f32>, spacing: f32) -> vec3<f32> {
    var d = vec3<f32>(0.0);
    if ninja_host.defaults.w > 0.5 {
        let raw = displacement(grid, spacing);
        let scale = jka_crest_scale_from_bounds(jka_crest_bounds, 1.0, 0.0, 3.0);
        d = vec3<f32>(raw.x * scale, raw.y, raw.z * scale);
    }
    let p = vec3<f32>(grid.x + d.x, d.y, grid.y + d.z);
    let uv = ninja_uv(p.xz);
    let wpo = ninja_ocean_wpo_cm(ninja_pd, ninja_tile1, ninja_tile2, ninja_clamp, ninja_repeat,
        uv, ninja_cm(p), ninja_cm(camera.position), vec3<f32>(0.0, 0.0, 1.0), camera.time,
        ninja_domain_share(uv));
    return d + vec3<f32>(wpo.x, wpo.z, -wpo.y) * 0.01;
}
@fragment
fn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let to_eye = camera.position - input.world;
    let v = normalize(to_eye);
    let p = ninja_cm(input.world);
    let eye = ninja_cm(camera.position);
    let uv = ninja_uv(input.world.xz);
    let pixel_depth = max(-(camera.view * vec4<f32>(curved(input.world), 1.0)).z * 100.0, 0.001);
    let basis = mat3x3<f32>(vec3<f32>(1.0,0.0,0.0),vec3<f32>(0.0,1.0,0.0),vec3<f32>(0.0,0.0,1.0));
    let sign_face = select(-1.0, 1.0, front);
    let attrs = ninja_ocean_shade(ninja_vd, ninja_pd, ninja_flow, ninja_rough, ninja_tile1, ninja_tile2,
        ninja_clamp, ninja_repeat, uv, p, pixel_depth, camera.time, sign_face, basis,
        NinjaOceanClassDefaults(ninja_host.defaults.x, ninja_host.defaults.y, ninja_host.defaults.z), ninja_domain_share(uv));
    // Reorient the authored flat-plane tangent normal to the actual displaced
    // triangle. This is the explicit FFT extension, not a second foam shader.
    let normal_cross = cross(dpdx(input.world), dpdy(input.world));
    var up = normalize(normal_cross * select(-1.0, 1.0, normal_cross.y >= 0.0) + vec3<f32>(0.0, 1e-7, 0.0));
    if abs(up.y) < 1e-5 { up = vec3<f32>(0.0, 1.0, 0.0); }
    let tangent = normalize(vec3<f32>(1.0, 0.0, 0.0) - up * up.x);
    let bitangent = cross(up, tangent);
    let n = normalize(tangent * attrs.normal.x + bitangent * attrs.normal.y + up * attrs.normal.z);
    if !jka_water_contains(input.grid) { discard; }
    let rough = max(attrs.roughness, 0.001); // avoid singular GGX at source underside roughness=0
    let coverage = saturate(attrs.opacity);
    let f0 = vec3<f32>(0.08 * attrs.specular);
    let gf = textureSampleLevel(ninja_gf, ninja_clamp, vec2<f32>(max(dot(n,v),0.0), rough), 0.0).rg;
    let brdf = river_slw_env_brdf(f0, gf);
    let l = sky_env.light_direction;
    let admission = jka_water_sky_admission(input.grid);
    let sun = sky_env.light_color * sky_env.light_intensity * admission;
    let shadow = water_sun_shadow(input.world, u32(surface.shadows.x)) * cloud_shadow(cloud_map, cloud_sampler, cloud_params, input.world);
    let ambient = env_irradiance(sky_atlas, sky_sampler, sky_env, n) * admission;
    let diffuse = attrs.base_color * coverage;
    let direct = river_slw_direct(n, v, l, diffuse, f0, rough);
    let size = vec2<f32>(textureDimensions(scene_depth));
    let screen = clamp(input.clip.xy / size, 0.55 / size, 1.0 - 0.55 / size);
    let raw = textureGather(scene_depth, wake_sampler, screen);
    let linear = 100.0 * camera.near * camera.far / (raw * (camera.far - camera.near) + camera.near);
    let gathered = river_slw_bilateral(screen, size, linear, vec4<f32>(1.0), pixel_depth, 0.0);
    let weights = gathered.weights * gathered.refraction_mask;
    let behind = vec3<f32>(dot(textureGather(0, scene_color, wake_sampler, screen), weights),
        dot(textureGather(1, scene_color, wake_sampler, screen), weights), dot(textureGather(2, scene_color, wake_sampler, screen), weights));
    let behind_depth = select(pixel_depth, gathered.scene_depth, dot(weights, vec4<f32>(1.0)) > 0.0);
    let behind_z = eye.z + (p.z - eye.z) * behind_depth / pixel_depth;
    let c = ninja_ocean_volume();
    let coeff = RiverSlwCoefficients(c.scattering, c.absorption, c.phase, c.color_scale);
    let vol = river_slw_volume(coeff, pixel_depth, behind_depth, behind, 1.0, p.z, behind_z,
        v, n, river_slw_ior(attrs.specular), l, sun, ambient, brdf, shadow, vec3<f32>(1.0), 1.0-coverage);
    let ray = reflect(-v,n);
    let sky = env_radiance(sky_atlas, sky_sampler, sky_env, ray, rough) * admission;
    var reflected = sky;
#ifdef JKA_NINJA_SSR
    let trace = screen_space_reflection(scene_color, wake_sampler, scene_depth,
        (camera.view * vec4<f32>(curved(input.world),1.0)).xyz,
        (camera.view * vec4<f32>(ray,0.0)).xyz, surface.reflections.x > 0.5,
        surface.reflections.y, REFLECTION_STEPS, (camera.view * vec4<f32>(n,0.0)).xyz, rough, TRACE_BELOW);
    reflected = mix(sky, trace.color, trace.confidence);
#endif
    let color = diffuse * ambient / 3.141592653589793 + (direct.diffuse + direct.specular) * sun * shadow
        + vol.luminance + reflected * brdf + attrs.emissive;
    return vec4<f32>(jka_surface_legacy_fog(apply_fog(color,input.world),input.world),1.0);
}
@fragment
fn ninja_wire(input: VertexOut) -> @location(0) vec4<f32> {
    if !jka_water_contains(input.grid) { discard; }
    return vec4<f32>(0.0, 0.9, 1.0, 1.0);
}
