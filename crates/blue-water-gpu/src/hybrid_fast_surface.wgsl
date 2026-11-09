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
#ifdef JKA_ADAPTIVE_HYBRID
    // Evaluate *material* LOD at the water's projected pixel footprint, not
    // distance from the camera or a block-sized render target. Wave geometry,
    // water/depth/shore classification and every output pixel stay unchanged.
    let span = max(length(dx), length(dy));
    // Most of upstream's additional slope work matters only when the screen
    // can resolve it. Smooth handoff avoids a camera-distance LOD ring.
    let fine_share = 1.0 - smoothstep(0.12, 0.7, span);
    var gust = 1.0;
    if fine_share > 0.001 {
        gust = mix(1.0, gust_gain(xz, span), fine_share);
    }
#endif
    var slope = vec2<f32>(0.0);
    var strain = vec3<f32>(0.0);
    var foam = 0.0;
    var variance = waves.tail.x;
    for (var i = 0; i < CASCADES; i++) {
        let level = log2(max(footprint * waves.texels[i], 1e-6));
        let detail = 1.0 - smoothstep(waves.info.x - 1.0, waves.info.x, level);
#ifdef JKA_ADAPTIVE_HYBRID
        if detail <= 0.0 {
            // Same variance as the fully filtered branch below, without the
            // scattered FFT derivative/extra texture reads. This is exact at
            // detail=0 and works independently per cascade and screen pixel.
            let kept = coast.scales[i];
            variance += waves.slopes[i] * kept * kept * gust;
            continue;
        }
#endif
        let copies = scatter(xz, i);
        // Keep the SAME scattered phases as displacement. Two fields, no
        // wider bubble-spread reads or procedural gust/breaking-group stack.
        let dr = copies_grad(field_derivatives, copies, i, dx / waves.tile[i], dy / waves.tile[i]);
        let er = copies_grad(field_extras, copies, i, dx / waves.tile[i], dy / waves.tile[i]);
        let kept = coast.scales[i];
        let d = mix_zero(dr, copies) * kept;
        let e = mix_mean(er, copies);
        let cross_strain = mix_zero(er, copies).x * kept;
#ifdef JKA_ADAPTIVE_HYBRID
        // Original Bluewater applies a spatial gust gain to *resolved* slope
        // samples; do not run its noise stack where the pixel cannot see it.
        slope += d.xy * detail * (1.0 - calm * waves.slick_shares[i]) * sqrt(gust);
#else
        slope += d.xy * detail * (1.0 - calm * waves.slick_shares[i]);
#endif
        strain += vec3<f32>(d.zw, cross_strain) * detail;
        foam = max(foam, e.y * kept * kept * detail);
#ifdef JKA_ADAPTIVE_HYBRID
        // The reference's stochastic multi-copy variance avoids a uniform
        // patchwork sheen when individual wave slopes are resolved. Only pay
        // for it in the visible fine-detail region.
        var filtered = max(e.z - dot(d.xy, d.xy), 0.0);
        if fine_share > 0.001 {
            filtered = mix(filtered, mix_variance(er, dr, copies), fine_share);
        }
        variance += mix(waves.slopes[i], filtered, detail) * kept * kept * gust;
#else
        variance += mix(waves.slopes[i], max(e.z - dot(d.xy, d.xy), 0.0), detail) * kept * kept;
#endif
    }
#ifdef JKA_ADAPTIVE_HYBRID
    // Port the original Bluewater's fine ripple read (wave_surface). This is
    // the extra sub-cascade field absent in the compact material; its footprint
    // fading and corresponding reduction of unresolved variance are preserved.
    // Only execute its explicit-gradient texture read where fine ripples are
    // actually screen-resolvable. No second render pass or pixel upscaling.
    if fine_share > 0.001 && CASCADES > 1 {
        let finest = CASCADES - 1;
        let level = max(log2(max(footprint * waves.tail.w, 1e-6)), 0.0);
        let shown = (1.0 - smoothstep(waves.info.x - 1.0, waves.info.x, level)) * fine_share;
        if shown > 0.0 {
            let read = textureSampleGrad(field_derivatives, field_sampler,
                xz / waves.tail.z + 0.5 / waves.info.z, finest,
                dx / waves.tail.z, dy / waves.tail.z);
            let kept = coast.scales[finest];
            let still = (1.0 - calm) * sqrt(gust);
            slope += read.xy * shown * still * kept;
            let drawn = -max(1.0 - level / waves.info.x, 0.0) * shown;
            variance += min(waves.slopes[finest], waves.tail.x) * drawn * (1.0 - calm) * gust * kept * kept;
        }
    }
    variance = max(variance, 0.0001);
#endif
    slope = slope * jka_hybrid.motion_mix.z + jka_hybrid_slope(xz);
    strain = (strain * jka_hybrid.motion_mix.z + jka_hybrid_strain(xz)) * jka_hybrid_crest_scale();
    let xx = 1.0 + strain.x;
    let zz = 1.0 + strain.y;
    let determinant = xx * zz - strain.z * strain.z;
    let normal = normalize(vec3<f32>(-(zz * slope.x - strain.z * slope.y),
        determinant, -(xx * slope.y - strain.z * slope.x)) + vec3<f32>(0.0, 1e-6, 0.0));
    return JkaFastWave(normal, foam / clamp(determinant, FOAM_JACOBIAN_MIN, FOAM_JACOBIAN_MAX), variance, determinant);
}

#ifdef JKA_ADAPTIVE_HYBRID
// Compact glints are *material* detail, not reduced pixel shading. This is a
// world-anchored Bernoulli sparkle distribution over the existing GGX lobe.
// Each cell is approximately one pixel across and we blend adjacent power-of-two
// footprints continuously as the camera moves. It is intentionally cheap.
fn jka_glint_hash(cell: vec2<i32>, salt: u32) -> f32 {
    var h = (bitcast<u32>(cell.x) * 0x8da6b343u)
        ^ (bitcast<u32>(cell.y) * 0xd8163841u) ^ (salt * 0xcb1ab31fu);
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return (f32(h >> 8u) + 0.5) / 16777216.0;
}

fn jka_glint_cell(xz: vec2<f32>, size: f32, probability: f32, time: f32) -> f32 {
    let cell = vec2<i32>(floor(xz / size));
    let clock = time * 2.5 + jka_glint_hash(cell, 17u);
    let slot = i32(floor(clock));
    // Fade from sample N to sample N+1. The previous version blended two
    // unrelated samples *within* N and popped at the slot boundary.
    let a = select(0.0, 1.0, jka_glint_hash(cell, bitcast<u32>(slot) * 977u + 31u) < probability);
    let b = select(0.0, 1.0, jka_glint_hash(cell, bitcast<u32>(slot + 1) * 977u + 31u) < probability);
    return mix(a, b, smoothstep(0.0, 1.0, fract(clock)));
}

fn jka_cheap_glints(xz: vec2<f32>, pixel_area: f32, time: f32) -> f32 {
    // Do NOT disable glints near the camera: the earlier smoothstep(0.1, 0.4)
    // made the close water completely sparkle-free. Use the projected pixel
    // footprint directly at every distance, with no material-LOD boundary.
    let span = sqrt(max(pixel_area, 1e-10));
    let lod = log2(max(span, 0.0009765625));
    let size = exp2(floor(lod));
    let probability = clamp(0.10 + 0.014 * max(lod, 0.0), 0.10, 0.23);
    let a = jka_glint_cell(xz, size, probability, time);
    let b = jka_glint_cell(xz, size * 2.0, probability, time);
    let draw = mix(a, b, smoothstep(0.0, 1.0, fract(lod)));
    // Bounded energy redistribution: a small part of the GGX sheen sparkles,
    // rather than amplifying the entire far sun lobe by 2-6x. Continuous
    // attenuation prevents a near/mid/far band (no hard cutoff at 24..64).
    // Expected gain is 1 in linear light, before fog and tonemapping.
    let strength = 0.26 / (1.0 + 0.30 * span);
    return 1.0 + strength * (draw / probability - 1.0);
}
#endif

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
    let wake_slope = -wake.normal.xz / max(wake.normal.y, 0.05) + jka_player_interaction(xz).yz;
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
    // The compact BRDF's broad environment lobe otherwise overexposes the
    // ocean horizon at night. Reduce ONLY sky-atlas radiance: moon/sun GGX,
    // traced SSR, water transmission, and foam keep their authored lighting.
    let sky = env_radiance(sky_atlas, sky_sampler, sky_env,
        normalize(vec3<f32>(direction.x, max(direction.y, 0.0), direction.z)), alpha)
        * (jka_admission * jka_hybrid.breaker.w);
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
    var sun = jka_fast_sun(up, view, sun_direction, sun_radiance, alpha) * beam;
#ifdef JKA_ADAPTIVE_HYBRID
    // Sparse sparkle redistributes only the GGX sunlight, never ambient,
    // sky/SSR, foam, or body light. No extra sun energy is introduced on average.
    // Derivatives must be evaluated in uniform control flow, even when the
    // glint itself is disabled in shadow or with the celestial light below the
    // horizon. Shadow/cloud admission still gates the final contribution.
    let grid_dx = dpdx(xz);
    let grid_dy = dpdy(xz);
    let pixel_area = abs(grid_dx.x * grid_dy.y - grid_dx.y * grid_dy.x);
    if sun_intensity > 0.0 && sun_direction.y > 0.0 && beam > 0.0 {
        sun *= jka_cheap_glints(xz, pixel_area, waves.wind.z);
    }
#endif
    var above = water + sun;
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
