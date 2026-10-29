// Opt-in Hybrid material: Godot-style cascade/foam accumulation, a single
// filtered environment lookup and GGX sunlight. Native FFT geometry, stochastic
// tile coordinates, admission, scene optics and underwater classification stay
// shared with the original material. No new render targets or per-frame bake.
struct JkaFastWave {
    normal: vec3<f32>,
    foam: f32,
    variance: f32,
    jacobian: f32,
}

fn jka_fast_wave(xz: vec2<f32>, calm: f32) -> JkaFastWave {
    let dx = dpdx(xz);
    let dy = dpdy(xz);
    let footprint = max(min(length(dx), length(dy)), max(length(dx), length(dy)) / ANISOTROPY);
    let coast = shore_at(xz);
    var slope = vec2<f32>(0.0);
    var strain = vec3<f32>(0.0);
    var foam = 0.0;
    var variance = waves.tail.x;
    for (var i = 0; i < CASCADES; i++) {
        let level = log2(max(footprint * waves.texels[i], 1e-6));
        let detail = 1.0 - smoothstep(waves.info.x - 1.0, waves.info.x, level);
        let copies = scatter(xz, i);
        // Keep the SAME scattered phases as displacement. Two fields, no
        // wider bubble-spread reads or procedural gust/breaking-group stack.
        let dr = copies_grad(field_derivatives, copies, i, dx / waves.tile[i], dy / waves.tile[i]);
        let er = copies_grad(field_extras, copies, i, dx / waves.tile[i], dy / waves.tile[i]);
        let kept = coast.scales[i];
        let d = mix_zero(dr, copies) * kept;
        let e = mix_mean(er, copies);
        let cross_strain = mix_zero(er, copies).x * kept;
        slope += d.xy * detail * (1.0 - calm * waves.slick_shares[i]);
        strain += vec3<f32>(d.zw, cross_strain) * detail;
        foam = max(foam, e.y * kept * kept * detail);
        variance += mix(waves.slopes[i], max(e.z - dot(d.xy, d.xy), 0.0), detail) * kept * kept;
    }
    slope = slope * jka_hybrid.motion_mix.z + jka_hybrid_slope(xz);
    strain = (strain * jka_hybrid.motion_mix.z + jka_hybrid_strain(xz)) * jka_hybrid_crest_scale();
    let xx = 1.0 + strain.x;
    let zz = 1.0 + strain.y;
    let determinant = xx * zz - strain.z * strain.z;
    let normal = normalize(vec3<f32>(-(zz * slope.x - strain.z * slope.y),
        determinant, -(xx * slope.y - strain.z * slope.x)) + vec3<f32>(0.0, 1e-6, 0.0));
    return JkaFastWave(normal, foam / clamp(determinant, FOAM_JACOBIAN_MIN, FOAM_JACOBIAN_MAX), variance, determinant);
}

// Godot's compact GGX distribution and Smith visibility, rather than the
// anisotropic physical glitter/Poisson micro-glint stack.
fn jka_fast_sun(up: vec3<f32>, view: vec3<f32>, light: vec3<f32>, radiance: vec3<f32>, alpha: f32) -> vec3<f32> {
    let half_vector = normalize(light + view + vec3<f32>(0.0, 1e-6, 0.0));
    let nv = max(dot(up, view), 0.00002);
    let nl = max(dot(up, light), 0.00002);
    let nh = max(dot(up, half_vector), 0.0);
    let a2 = alpha * alpha;
    let denominator = nh * nh * (a2 - 1.0) + 1.0;
    let distribution = a2 / max(3.14159265359 * denominator * denominator, 1e-6);
    let visibility = 2.0 * nl / max(nl * sqrt(a2 + (1.0 - a2) * nv * nv)
        + nv * sqrt(a2 + (1.0 - a2) * nl * nl), 1e-5);
    let fresnel = 0.02 + 0.98 * pow(1.0 - nv, 5.0);
    // Match Godot's 4*NdotV + 0.1 denominator: the ordinary GGX form
    // otherwise produces an excessive white sheen at grazing angles.
    return radiance * (fresnel * distribution * visibility * 0.25 * nv / (nv + 0.025));
}

fn jka_fast_patterns(xz: vec2<f32>, stretch: f32, sun: vec3<f32>) -> FoamPatterns {
    if jka_hybrid.breaker.z > 0.5 {
        return foam_patterns(foam_map, foam_sampler, surface.foam.z, xz, waves.sea.z, stretch, sun);
    }
    let heading = vec2<f32>(cos(waves.sea.z), sin(waves.sea.z));
    let uv = vec2<f32>(dot(xz, heading) / stretch, dot(xz, vec2<f32>(-heading.y, heading.x))) / 40.0;
    let pattern = textureSample(foam_map, foam_sampler, uv);
    let blur = smoothstep(0.5, 3.0, max(length(dpdx(uv)), length(dpdy(uv))) * 512.0);
    return FoamPatterns(pattern, blur, pattern.r, blur, pattern.b, pattern.g, pattern, blur, pattern.a);
}

@fragment
fn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let xz = input.grid;
    let footprint = max(length(dpdx(xz)), length(dpdy(xz)));
    let wake = wake_surface(xz, input.world, footprint);
    let sample = jka_fast_wave(xz, wake.slick);
    let to_camera = camera.position.xyz - input.world;
    let distance = max(length(to_camera), 1e-5);
    let view = to_camera / distance;
    let pixel_size = distance * 2.0 * camera.tan_half_fov_y * camera.viewport.w / sqrt(max(abs(view.y), 0.05));
    let rain = rain_surface(xz, waves.wind.z, surface.weather.x, pixel_size);
    let wake_slope = -wake.normal.xz / max(wake.normal.y, 0.05);
    var up = normalize(sample.normal - vec3<f32>(wake_slope.x + rain.slope.x, 0.0,
        wake_slope.y + rain.slope.y) * abs(sample.normal.y));
    if dot(up, view) < 0.0 { up = -up; }
    let alpha = clamp(sqrt(max(sample.variance + rain.variance + wake.variance, 0.0025)), 0.05, 0.6);
    let reflectance = 0.02 + 0.98 * pow(1.0 - max(dot(up, view), 0.0), 5.0);
    let direction = reflect(-view, up);
    let sun_direction = sky_env.light_direction;
    let sun_intensity = sky_env.light_intensity;
    let sun_radiance = sky_env.light_color * sun_intensity;
    let shadow = water_sun_shadow(input.world, u32(surface.shadows.x)) * cloud_shadow(cloud_map, cloud_sampler, cloud_params, input.world);
    let lit = mix(1.0, shadow, clamp(sun_intensity / 2.0, 0.0, 1.0));
    let beam = clamp((shadow - CLOUD_SHADOW_FLOOR) / (1.0 - CLOUD_SHADOW_FLOOR), 0.0, 1.0);
    let daylight = env_irradiance(sky_atlas, sky_sampler, sky_env, vec3<f32>(0.0, 1.0, 0.0));
    let sky = env_radiance(sky_atlas, sky_sampler, sky_env,
        normalize(vec3<f32>(direction.x, max(direction.y, 0.0), direction.z)), alpha) * jka_admission;
    var reflected = sky;
#ifdef JKA_FAST_SSR
    let to_view = camera.view;
    let traced = screen_space_reflection(scene_color, wake_sampler, scene_depth,
        (to_view * vec4<f32>(curved(input.world), 1.0)).xyz,
        (to_view * vec4<f32>(direction, 0.0)).xyz, surface.reflections.x > 0.5,
        surface.reflections.y, REFLECTION_STEPS,
        (to_view * vec4<f32>(up, 0.0)).xyz, alpha, TRACE_BELOW);
    reflected = mix(sky, traced.color, traced.confidence);
#endif
    let pixel = vec2<i32>(input.clip.xy);
    let depth = textureLoad(scene_depth, pixel, 0);
    let scene_view_z = -camera.near * camera.far / (depth * (camera.far - camera.near) + camera.near);
    let ray_view_z = (camera.view * vec4<f32>(-view, 0.0)).z;
    let receiver_range = scene_view_z / min(ray_view_z, -1e-4);
    let column = select(max(receiver_range - distance, 0.0), OPEN_DEEP, depth <= 0.0 || receiver_range + 0.03 < distance);
    let optics = sample_water(surface.water, xz, surface.absorption.rgb);
    let absorption = optics.absorption;
    let backscatter = optics.backscatter;
    let attenuation = select(absorption, water_beam(optics) - backscatter, surface.water.absorption.w > 0.5);
    let deep = upwelling(subsurface_reflectance(absorption, backscatter), sun_direction, sun_radiance, daylight, lit)
        * (surface.transmission.rgb / JKA_FAST_TRANSMISSION);
    let bent = jka_refracted_scene(input.world, -view, up, 1.0 / 1.333, column * 2.0, true);
    let transmitted = select(textureLoad(scene_color, pixel, 0).rgb, bent.rgb, bent.a >= 0.0);
    let through = select(column, bent.a, bent.a >= 0.0);
    let body = water_body(transmitted, deep, column_transmittance(attenuation, backscatter, through, view));
    let water = mix(body, reflected, reflectance);
    let patterns = jka_fast_patterns(xz, surface.foam.x * 2.0 + 1.0, sun_direction);
    let pattern = patterns.pattern;
    let blur = patterns.blur;
    let foam_light = foam_radiance(daylight / 3.14159265359, sun_radiance, up, sun_direction, lit);
    let cover = max(sample.foam * surface.crest_foam.a, wake.foam);
    let crest_foam = foam_share(cover, pattern.r, blur, 0.08);
    let shore_foam = foam_share(pow(1.0 - smoothstep(0.0, SHORE_DEPTH, column), 2.0) * SHORE_COVER,
        pattern.r, blur, SHORE_EDGE) * surface.shoreline_foam.a;
    let windrow = windrow_opacity(surface.foam.y, patterns.lines, max(surface.foam.z, 0.001),
        patterns.gather, pattern.r, patterns.streaks, max(blur, patterns.streaks_blur), sample.jacobian);
    // Preserve the simulated gale streaks and their broad patch positions.
    // Reuse sampled lace instead of evaluating extra world-space tear noise.
    let old = textureSampleGrad(field_displacement, field_sampler, field_uv(xz, 1), 1,
        dpdx(xz) / waves.tile.y, dpdy(xz) / waves.tile.y).w;
    let wind = waves.wind.xy;
    let patches = value_noise(vec2<f32>(dot(xz, wind) / STREAK_PATCH_ALONG,
        dot(xz, vec2<f32>(-wind.y, wind.x)) / STREAK_PATCH_ACROSS), 907u);
    let streaks = gale_streaks(old * smoothstep(0.25, 0.6, patches), patterns.streaks, pattern.r,
        max(blur, patterns.streaks_blur), 0.5);
    let windrows = clamp(max(windrow, streaks) * surface.crest_foam.a * jka_hybrid.breaker.y, 0.0, 1.0);
    let foam = clamp(max(crest_foam, shore_foam) * jka_hybrid.breaker.y, 0.0, 1.0);
    var above = water + jka_fast_sun(up, view, sun_direction, sun_radiance, alpha) * beam;
    above = foam_over(above, water, foam_light * surface.surface_foam.rgb * thin_foam(absorption), reflected, reflectance, windrows);
    above = mix(above, foam_light * mix(surface.crest_foam.rgb, surface.shoreline_foam.rgb, shore_foam), clamp(foam, 0.0, 1.0));
    var color = jka_surface_legacy_fog(apply_fog(above, input.world), input.world);
    let submerged = select(surface.pigment.w > 0.5 && textureLoad(underwater_probe, vec2<i32>(0, 0), 0).g > 0.0,
        surface.pigment.w > 1.5, jka_hybrid.breaker.x > 0.5);
    if !front && submerged {
        let pigment = select(surface.pigment.rgb, water_pigment(optics, surface.absorption.rgb, surface.pigment.rgb), surface.water.absorption.w > 0.5);
        color = underside(view, -up, pigment, select(absorption, water_beam(optics), surface.water.absorption.w > 0.5), sun_direction, sky_env.light_color);
        let window_scene = jka_refracted_scene(input.world, -view, up, 1.333, 100.0, false);
        let window_ray = refract(-view, up, 1.333);
        if window_scene.a >= 0.0 {
            color += (window_scene.rgb - sky_reflection(window_ray, 0.0)) * WINDOW_GAIN * (1.0 - water_fresnel(max(dot(window_ray, -up), 0.0)));
        }
    }
    return vec4<f32>(color, 1.0);
}
