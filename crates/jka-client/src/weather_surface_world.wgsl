// Forward weather shading shared by bsp.wgsl and bsp_lean.wgsl. It relies on the
// world shader's module-scope bindings (material, camera, weather_surface,
// weather_occlusion_height, lighting_settings, reflection_probe_*, shadow_settings)
// and on weather_surface.wgsl for the puddle shape, ripples and water BRDF.

// Specialized per pipeline from the cold-path WorldShaderVariantKey. Dry scenes
// compile the whole wetness/puddle/ripple path out instead of skipping it with a
// runtime uniform test, which keeps its registers and code out of every fragment.
// Defaults to on so a pipeline built without the constant keeps full behavior.
override ENABLE_WEATHER_SURFACE: bool = true;

// How much a material's own gloss shows through a wet film. Grass, snow, foliage
// and cloth stay matte, earth stays soft, metal, glass, stone and plastic shine.
fn weather_material_gloss_response() -> f32 {
    let material_kind = (material.header.w >> 8u) & 31u;
    if (material_kind == 5u || material_kind == 6u || material_kind == 14u
        || material_kind == 19u || material_kind == 20u || material_kind == 21u
        || material_kind == 22u || material_kind == 27u) { return 0.28; }
    if (material_kind == 7u || material_kind == 8u || material_kind == 9u || material_kind == 17u) { return 0.46; }
    if (material_kind == 3u || material_kind == 4u || material_kind == 10u
        || material_kind == 12u || material_kind == 15u || material_kind == 18u
        || material_kind == 25u || material_kind == 26u || material_kind == 29u
        || material_kind == 30u || material_kind == 31u) { return 1.0; }
    return 0.78;
}

struct WeatherSurfaceResponse {
    wetness: f32,            // damp film, damp margin or standing water
    puddle: f32,             // standing-water coverage
    depth: f32,              // standing-water depth, 0 at the shore
    rim: f32,                // wet shoreline band just outside the water
    runoff: f32,             // rivulet coverage on surfaces too steep to pool
    runoff_tilt: f32,        // signed across-slope tilt of the rivulet wall
    water_normal: vec3<f32>, // ripple-perturbed water surface; straight up when dry
};

// Rain that lands on a slope or a wall cannot pool, so it runs off. Spacing of the
// rivulets across the slope and the length of the pattern along it, in world units
// at 40 to the metre: 11 units is a 27 cm gap between streaks and 260 units a 6.5 m
// run, and it is that ratio that stretches the noise into streaks instead of blobs.
const WEATHER_RUNOFF_ACROSS: f32 = 11.0;
const WEATHER_RUNOFF_ALONG: f32 = 260.0;
// Units per second the pattern travels downhill. 55 is about 1.4 m/s: fast enough
// to read as running water, slow enough not to strobe against the frame rate.
const WEATHER_RUNOFF_SPEED: f32 = 55.0;
// How hard a rivulet's flank tilts the shading normal. Sampled over a wall, the
// across-slope difference below has a median of 0.15, a 95th percentile of 0.56 and
// a maximum of 0.85, so a typical flank tilts about 4 degrees, a strong one 14 and
// the steepest 21. Those are the figures only because the tilt is gated by
// `runoff_presence` rather than scaled by it.
const WEATHER_RUNOFF_TILT: f32 = 0.45;
// A streak with no internal structure is a smooth gradient, and the eye barely
// registers a smooth gradient moving. These are the fine octave it does catch:
// 3.5 units (9 cm) across, scrolling 4.5x faster, so water visibly travels inside
// the rivulet instead of the whole band sliding as one piece.
const WEATHER_RUNOFF_FINE: f32 = 3.1;
const WEATHER_RUNOFF_FINE_ALONG: f32 = 58.0;
const WEATHER_RUNOFF_FINE_SPEED: f32 = 4.5;
const WEATHER_RUNOFF_FINE_GAIN: f32 = 0.45;
// Half the across-slope spacing of the central difference, in noise cells.
const WEATHER_RUNOFF_SAMPLE: f32 = 0.35;

// The facet the fragment actually sits on, from the screen-space derivatives of its
// world position. A BSP surface carries smoothed vertex normals, so the interpolated
// normal is a shading convenience and not the geometry: on a shallow slant the
// smoothing can tilt it further than the facet is tilted, which points the gravity
// tangent somewhere the surface does not fall - up to and including uphill. Water
// runs down the geometry.
//
// Both call sites are in uniform control flow, which is what the derivative builtins
// require: `weather_surface_response` is past only uniform tests when it asks, and
// the debug view is selected by a uniform. Derivative winding promises nothing about
// which side is out, so the sign is matched to the shading normal.
fn weather_facet_normal(world: vec3<f32>, shading: vec3<f32>) -> vec3<f32> {
    let facet = cross(dpdx(world), dpdy(world));
    let facet_length = length(facet);
    if (facet_length < 1.0e-9) {
        return shading;
    }
    let unit_facet = facet / facet_length;
    return select(-unit_facet, unit_facet, dot(unit_facet, shading) >= 0.0);
}

// Evaluated once per fragment: every later consumer (albedo, roughness, coat,
// specular, PBR) reads this instead of repeating the field fetch and ripple maths.
fn weather_surface_response(input: VertexOut) -> WeatherSurfaceResponse {
    var result: WeatherSurfaceResponse;
    result.wetness = 0.0;
    result.puddle = 0.0;
    result.depth = 0.0;
    result.rim = 0.0;
    result.runoff = 0.0;
    result.runoff_tilt = 0.0;
    result.water_normal = vec3<f32>(0.0, 1.0, 0.0);
    if (!ENABLE_WEATHER_SURFACE
        || (material.header.z & 4u) == 0u
        || (weather_surface.amount_distance.x <= 0.001 && weather_surface.puddle.x <= 0.001)) {
        return result;
    }

    let world = input.world_position;
    let distance_to_surface = length(world - camera.camera_pos_time.xyz);
    let fade_start = weather_surface.film_fade.x;
    let distance_weight = 1.0 - smoothstep(
        fade_start,
        max(weather_surface.film_fade.y, fade_start + 1.0),
        distance_to_surface
    );

    let normal = normalize(input.world_normal);
    let normal_y = normal.y;
    // Runoff only. `orientation` and `upright` below decide where puddles are, and
    // puddle placement is mirrored in weather/cpu_field.rs and has to stay a pure
    // function of world position - it cannot start depending on a derivative.
    let facet_normal = weather_facet_normal(world, normal);

    var field = weather_default_field();
    if (weather_surface.occlusion_size.z != 0u) {
        field = weather_sample_field(
            weather_occlusion_height,
            world,
            weather_surface.occlusion_uv.xy,
            weather_surface.occlusion_uv.zw,
            normal_y
        );
    }

    let orientation = smoothstep(-0.45, 0.12, normal_y);
    var wetness = clamp(weather_surface.amount_distance.x * weather_surface.amount_distance.w
        * distance_weight * field.exposure * orientation, 0.0, 1.0);

    // How level the face is. Puddles need it and runoff is its exact complement, so
    // every drop of rain either pools where it lands or runs off, and a slope in
    // between does a little of each.
    let upright = smoothstep(0.75, 0.95, normal_y);

    let accumulation = clamp(weather_surface.puddle.x, 0.0, 1.0);
    if (accumulation > 0.001 && field.exposure > 0.001) {
        let shape = weather_puddle_shape(field, world.xz, accumulation, weather_surface.look.x);
        // Puddle placement is persistent world state: no camera-distance weight.
        // Walking toward or away from a pool must never create/remove it. A wall
        // that rises out of a flat floor is not part of the water.
        let coverage = shape.coverage * field.exposure * upright;
        result.puddle = coverage;
        result.depth = shape.depth;
        result.rim = shape.damp * (1.0 - coverage) * field.exposure * upright;
        // Standing water and its damp margin stay wet after the film has dried,
        // which lets puddles linger naturally after the rain stops.
        wetness = max(wetness, max(coverage * 0.96, shape.damp * field.exposure * upright * 0.85));
        if (coverage > 0.001) {
            // Ripples die away toward the shore instead of being cut by it.
            // look.z (high quality) also buys the fine ring layers, and those carry
            // enough gradient that the old 0.12 gain would have flattened them back
            // out. A still mirror reads as glass, not water: this is the tilt that
            // makes the reflection move. Their ~10 cm wavelength goes sub-pixel
            // somewhere past 70 m, where it would only shimmer in a mirror-smooth
            // reflection, so fade them (and the extra gain with them) before then.
            let detail = weather_surface.look.z
                * (1.0 - smoothstep(1200.0, 2800.0, distance_to_surface));
            let gradient = (weather_water_gradient(
                world.xz,
                weather_surface.puddle.y,
                clamp(weather_surface.puddle.z, 0.0, 1.0),
                weather_surface.wind.xy,
                detail
            ) + weather_wake_gradient(world)) * smoothstep(0.0, 0.5, shape.depth);
            result.water_normal = weather_water_normal(gradient, mix(0.07, 0.11, detail));
        }
    }

    // Runoff. Everything above is flat-only: `upright` gates puddles to near-level
    // faces, so a wall in the rain used to get darker and glossier and nothing else.
    // Water that lands where it cannot pool runs down instead, so the complement of
    // that gate gets gravity-aligned rivulets. Rain that has stopped leaves no
    // runoff even though the wall stays wet for a while, which is what `puddle.z`
    // (ripple strength, zero when the rain is off) buys here.
    let falling = clamp(weather_surface.puddle.z, 0.0, 1.0);
    // Rivulets are a 27 cm feature: fade them before they go sub-pixel and shimmer.
    let runoff_detail = 1.0 - smoothstep(1800.0, 3600.0, distance_to_surface);
    // Presence, not amplitude. Scaling the rivulets *by* this was what made them
    // invisible: on a vertical wall in ordinary rain it sits at 0.45, so the mask
    // peaked at 0.45 and the flank tilt - already the noise gradient, which is worth
    // 4 to 21 degrees on its own - came out at 1.7 degrees. Fading in over
    // 0.03..0.35 still has rivulets appear and die with the rain, dry out with the
    // surface and fade with distance, without quietly halving everything they drive.
    let runoff_presence =
        smoothstep(0.03, 0.35, (1.0 - upright) * falling * wetness * runoff_detail);
    if (runoff_presence > 0.004) {
        // Down the slope, and across it. `flow` and `normal` are orthonormal, so
        // the cross product is already unit length.
        let gravity = vec3<f32>(0.0, -1.0, 0.0);
        let along = gravity - facet_normal * dot(gravity, facet_normal);
        let along_length = length(along);
        if (along_length > 0.05) {
            let flow = along / along_length;
            let across = cross(facet_normal, flow);
            // Scrolling the along coordinate (not the time) moves the whole pattern
            // downhill. The sine is a meander: without it every rivulet sits on the
            // noise lattice and the wall wears a perfectly regular comb, which is
            // the banding `weather_relief` warns about in its own comment.
            let along_coordinate =
                dot(world, flow) - weather_surface.puddle.y * WEATHER_RUNOFF_SPEED;
            let across_coordinate =
                dot(world, across) + sin(along_coordinate * 0.013) * 5.0;
            let p = vec2<f32>(
                across_coordinate / WEATHER_RUNOFF_ACROSS,
                along_coordinate / WEATHER_RUNOFF_ALONG
            );
            // One central difference across the slope answers both questions from
            // two lookups: its mean is how much water this column carries and its
            // difference is the across-slope slope, which is the one that tilts a
            // rivulet's flank into the light. Along-slope structure needs no
            // gradient - a streak is meant to be flat along its own length.
            let left = weather_value_noise(p - vec2<f32>(WEATHER_RUNOFF_SAMPLE, 0.0));
            let right = weather_value_noise(p + vec2<f32>(WEATHER_RUNOFF_SAMPLE, 0.0));
            // Measured over a wall this pair of samples has a mean of 0.48 and a
            // deviation of 0.20. The window keeps about a quarter of the surface
            // carrying a rivulet - separate streams down a wall, not a sheet - and
            // is deliberately narrow: the original 0.52..0.86 produced soft blobs
            // whose edges the eye reads as shading rather than as water, and a
            // rivulet in life has a defined edge.
            var ridge = smoothstep(0.56, 0.74, (left + right) * 0.5);
            // Near field only: the fine octave is the first thing to go sub-pixel,
            // and `runoff_detail` already measures exactly that.
            let fine = weather_value_noise(vec2<f32>(
                p.x * WEATHER_RUNOFF_FINE,
                (dot(world, flow)
                    - weather_surface.puddle.y * WEATHER_RUNOFF_SPEED * WEATHER_RUNOFF_FINE_SPEED)
                    / WEATHER_RUNOFF_FINE_ALONG
            ));
            ridge = ridge * (1.0 - WEATHER_RUNOFF_FINE_GAIN * runoff_detail * (1.0 - fine));
            result.runoff = ridge * runoff_presence;
            result.runoff_tilt = (right - left) * WEATHER_RUNOFF_TILT * runoff_presence;
        }
    }

    result.wetness = wetness;
    return result;
}

// How much water a surface drinks. Porous materials trap light in their pores and
// lose a large share of their diffuse reflectance; sealed ones just carry a film on
// top and barely change colour at all. This is deliberately not the inverse of the
// gloss response above: wet concrete both darkens a lot and turns glossy.
fn weather_material_porosity() -> f32 {
    let material_kind = (material.header.w >> 8u) & 31u;
    // Sealed: metal, glass, marble, ice, bulletproof glass, rubber, plastic, tiles,
    // shatterglass, armour, screens.
    if (material_kind == 3u || material_kind == 4u || material_kind == 10u
        || material_kind == 12u || material_kind == 15u || material_kind == 18u
        || material_kind == 24u || material_kind == 25u || material_kind == 26u
        || material_kind == 29u || material_kind == 30u || material_kind == 31u) { return 0.15; }
    // Thirsty: wood, dirt, sand, gravel, concrete, mud, dry leaves, fabric, canvas,
    // rock, carpet, plaster. These are the surfaces that visibly change colour in
    // the rain, and the ground a player spends the whole match looking at.
    if (material_kind == 1u || material_kind == 2u || material_kind == 7u
        || material_kind == 8u || material_kind == 9u || material_kind == 11u
        || material_kind == 17u || material_kind == 19u || material_kind == 21u
        || material_kind == 22u || material_kind == 23u || material_kind == 27u
        || material_kind == 28u) { return 1.0; }
    // Grass, leaves, snow, flesh - and anything a shader never classified, which on
    // a world surface is usually stone.
    return 0.6;
}

// Standing water or a rivulet: how much of the fragment is liquid water rather than
// a damp film. The two are complementary by construction - a face level enough to
// pool is not steep enough to shed - so a max is the whole story, and every response
// that used to key off `puddle` alone keys off this instead. That is what tells a
// wet wall apart from a wall with water running down it.
fn weather_water_like(weather: WeatherSurfaceResponse) -> f32 {
    return max(weather.puddle, clamp(weather.runoff, 0.0, 1.0));
}

// Water filling the pores traps light, so the surface loses a share of its diffuse
// reflectance. That share depends on how thirsty the material is, not on how bright
// it happened to be: the luminance power curve this used to apply did the opposite,
// stripping 65% from near-black asphalt while taking only 14% off pale sand, which
// is backwards - wet sand is one of the most obviously wet things there is. A
// bounded multiply is also what the PBR wetness references use. Only brightness
// changes, so a wet wall never takes on a colour of its own.
fn weather_wet_albedo(dry: vec3<f32>, weather: WeatherSurfaceResponse) -> vec3<f32> {
    let porosity = weather_material_porosity();
    let soak = clamp(weather.wetness, 0.0, 1.0);
    var darkening = 1.0 - 0.52 * porosity * soak;
    // Standing water, and the damp ring just outside it, soak harder still.
    darkening *= 1.0 - 0.22 * porosity * clamp(weather.puddle + weather.rim * 0.5, 0.0, 1.0);
    // A rivulet needs a coefficient of its own, and it must not be scaled by
    // porosity the way the soak terms are. Porosity is about water filling pores; a
    // film *running over* a surface darkens it by replacing the air-to-surface
    // interface with a water-to-surface one, which happens on glass and metal too -
    // rain streaks on a window are not subtle. Keying it off porosity meant an
    // unclassified wall (0.6, which is most of them) got 60% of an already modest
    // number: measured off a screenshot of a wall in rain, the streaks came out at
    // 10% of the wall's brightness peak to peak, where they need something like 28%
    // to read as water rather than as a smudge in the lighting.
    darkening *= 1.0 - 0.34 * mix(0.65, 1.0, porosity) * clamp(weather.runoff, 0.0, 1.0);
    return dry * max(darkening, 0.08);
}

// Ripples read even where the water has little to reflect: a slope facing the
// light is a touch brighter, one facing away a touch darker, exactly as a
// normal-mapped surface would shade.
fn weather_ripple_relief(weather: WeatherSurfaceResponse) -> f32 {
    if (weather.puddle <= 0.001) {
        return 1.0;
    }
    let toward_light = -weather_surface.sun_direction.xz;
    let light_length = length(toward_light);
    if (light_length < 0.05) {
        return 1.0;
    }
    let slope = dot(weather.water_normal.xz, toward_light / light_length);
    return 1.0 + clamp(slope * 2.6, -0.32, 0.32) * smoothstep(0.05, 0.85, weather.puddle);
}

// High-quality water takes on the cool tint of the ocean's body colour as it gets
// deeper, so a deep pool reads as deeper than a wet patch.
fn weather_water_depth_tint(weather: WeatherSurfaceResponse) -> vec3<f32> {
    if (weather_surface.look.z < 0.5) {
        return vec3<f32>(1.0);
    }
    return mix(vec3<f32>(1.0), vec3<f32>(0.78, 0.88, 1.0), weather.puddle * weather.depth);
}

fn weather_wet_roughness(dry_roughness: f32, weather: WeatherSurfaceResponse) -> f32 {
    if (!ENABLE_WEATHER_SURFACE) {
        return clamp(dry_roughness, 0.01, 1.0);
    }
    let gloss_amount = pow(clamp(weather.wetness, 0.0, 1.0), 1.35);
    let film_roughness = mix(dry_roughness, max(0.12, dry_roughness * 0.28),
        gloss_amount * weather_material_gloss_response());
    // Inside the wet mask the smoothness is overridden toward a true water-film
    // response instead of merely nudging the original material roughness.
    var wet = mix(film_roughness, 0.012, smoothstep(0.05, 0.80, weather.puddle));
    // A rivulet is a water film as much as a puddle is, and this is what turns the
    // sun's answer on it from a broad dim sheen into a tight bright streak. Floored
    // well above a puddle's 0.012 and only ever allowed to smooth, never roughen: a
    // streak a few pixels wide at mirror smoothness resolves a light to single
    // sparkling pixels, which is the aliasing the local-light lobe below already
    // carries a floor against.
    wet = mix(wet, min(wet, 0.06), clamp(weather.runoff, 0.0, 1.0));
    return clamp(wet, 0.01, 1.0);
}

// Rivulets tilt the surface they run down instead of replacing it: the caller's
// normal may be a mapped one, and a brick wall in the rain still has bricks. The
// tilt is carried as a scalar and rebuilt in the caller's own frame, which is a
// handful of ALU and no second noise lookup.
fn weather_runoff_normal(n: vec3<f32>, weather: WeatherSurfaceResponse) -> vec3<f32> {
    if (weather.runoff_tilt == 0.0) {
        return n;
    }
    let gravity = vec3<f32>(0.0, -1.0, 0.0);
    let along = gravity - n * dot(gravity, n);
    let along_length = length(along);
    if (along_length < 0.05) {
        return n;
    }
    return normalize(n + cross(n, along / along_length) * weather.runoff_tilt);
}

// Standing water is a new horizontal surface laid over the material, so it must
// not inherit the authored/smoothed material normal. Water wins over runoff where
// a gentle slope carries both: a rivulet running into a pool stops being a rivulet.
fn weather_reflection_normal(base_normal: vec3<f32>, weather: WeatherSurfaceResponse) -> vec3<f32> {
    let n = normalize(base_normal);
    if (!ENABLE_WEATHER_SURFACE) {
        return n;
    }
    if (weather.puddle <= 0.001) {
        return weather_runoff_normal(n, weather);
    }
    let water_mask = smoothstep(0.05, 0.85, weather.puddle);
    return normalize(mix(n, weather.water_normal, water_mask * 0.985));
}

// The environment radiance a wet surface reflects, kept split by source so the
// diagnostic views can show each one on its own.
struct WeatherCoatEnvironment {
    radiance: vec3<f32>, // what the coat actually reflects
    probe: vec3<f32>,    // reflection probe: the geometry standing around the surface
    sky: vec3<f32>,      // the map's own skybox along the mirror ray
    sky_share: f32,      // 0 probe only, 1 skybox only
};

// How much of the coat's environment comes from the skybox rather than the probe.
// For a level puddle `reflection_y` is sin(the camera's elevation above the water),
// so it maps straight onto viewing distance: at a 54-unit eye height a puddle 50
// units away sits at 0.73, one 200 units away at 0.26 and one 400 units away at
// 0.13. A steep mirror ray really does leave for the sky and a grazing one really
// does run into the geometry the probe holds, so the handover is the right idea.
// The window used to end at 0.16 - about 9.2 degrees - which made every puddle
// past roughly 350 units pure skybox and left the probe unused in normal play.
// The ceiling exists because the skybox carries no occlusion of its own: a puddle
// that discards the probe entirely cannot show the wall standing next to it, which
// is what makes it read as a pale wash of sky rather than water.
//
// `film_fade.z/.w` carry both values so `puddle_sky` can tune them in-game; the
// defaults are RAIN_SKY_BLEND_* in weather/rain.rs.
fn weather_coat_sky_share(reflection_y: f32) -> f32 {
    let window_end = max(weather_surface.film_fade.z, 0.02);
    let ceiling = clamp(weather_surface.film_fade.w, 0.0, 1.0);
    return ceiling * smoothstep(0.0, window_end, reflection_y);
}

fn weather_coat_environment(reflection: vec3<f32>, wetness: f32, puddle: f32) -> WeatherCoatEnvironment {
    var result: WeatherCoatEnvironment;
    let water = smoothstep(0.05, 0.80, puddle);
    let jka_reflection = normalize(vec3<f32>(reflection.x, -reflection.z, reflection.y));
    let wet_lod = mix(0.25, 0.12, pow(clamp(wetness, 0.0, 1.0), 1.25));
    let lod_fraction = mix(wet_lod, 0.0, water);
    result.probe = textureSampleLevel(reflection_probe_texture, reflection_probe_sampler,
        jka_reflection, lod_fraction * f32(max(textureNumLevels(reflection_probe_texture), 1u) - 1u)).rgb;
    result.sky = result.probe;
    result.sky_share = 0.0;
    if (weather_surface.look.z > 0.5 && weather_surface.sun_radiance.w > 0.5) {
        // High quality water mirrors the map's own sky. Reflection quality gates the
        // probe cubemap and SSR, but the skybox is bound to every surface, so a wet
        // street or puddle can always show the real sky, handing back to the probe
        // toward the horizon where the ground and buildings would be in the way.
        result.sky = sample_map_sky_renderer(reflection, mix(0.30, 0.04, water));
        result.sky_share = weather_coat_sky_share(reflection.y);
    }
    result.radiance = mix(result.probe, result.sky, result.sky_share);
    return result;
}

// Fresnel times the film/water weighting: the fraction of the environment radiance
// that leaves the surface. Split out so the diagnostic views can reuse it.
fn weather_coat_strength(weather: WeatherSurfaceResponse, n_dot_v: f32) -> f32 {
    var fresnel = 0.025 + 0.975 * pow(1.0 - n_dot_v, 5.0);
    if (weather_surface.look.z > 0.5) {
        // High quality water uses the GodotOcean roughness-aware Fresnel.
        fresnel = water_fresnel(n_dot_v, min(weather_wet_roughness(0.65, weather), 0.5));
    }
    // The film response is what a *damp* surface reflects - 0.40 on a wet stone wall
    // against 0.92 for water - and sending runoff through it was the largest single
    // reason a rivulet could not be seen. Water running down a wall is standing water
    // that happens to be moving, so it answers as water.
    let water_like = weather_water_like(weather);
    let coat_amount = max(
        pow(clamp(weather.wetness, 0.0, 1.0), 1.45),
        max(smoothstep(0.02, 0.75, weather.puddle), clamp(weather.runoff, 0.0, 1.0) * 0.9)
    );
    let film_response = 0.34 * weather_material_gloss_response() * weather_surface.look.w;
    let water_response = 0.92;
    return fresnel * coat_amount * mix(film_response, water_response, water_like);
}

fn weather_surface_coat(input: VertexOut, weather: WeatherSurfaceResponse) -> vec3<f32> {
    let wetness = weather.wetness;
    let puddle = weather.puddle;
    if ((lighting_settings.feature_flags.w & 255u) < 1u || (wetness <= 0.001 && puddle <= 0.001)) {
        return vec3<f32>(0.0);
    }
    let n = weather_reflection_normal(input.world_normal, weather);
    let v = normalize(camera.camera_pos_time.xyz - input.world_position);
    let reflection = normalize(reflect(-v, n));
    let environment = weather_coat_environment(reflection, wetness, puddle);
    return environment.radiance * weather_coat_strength(weather, max(dot(n, v), 0.0));
}

// Diagnostic views for wetness and the puddle coat, selected by `puddle_debug
// <mode>` and replacing the lit colour outright:
//   1  coverage:    grey dry, green wet film, blue standing water
//   2  coat source: red is pure skybox, green is pure reflection probe
//   3  the probe radiance the coat would reflect, unweighted
//   4  the skybox radiance the coat would reflect, unweighted
//   5  runoff: red is rivulet coverage, blue the across-slope tilt it applies
//   6  material: red is porosity, green is the gloss response, blue a weather
//      receiver at all. A surface no shader gave a MATERIAL_* id reads 0.6/0.78.
//   7  the runoff's own share of the albedo darkening, at 3x so it is readable:
//      full white is a third darker. This is the number to compare against a
//      screenshot, because it is the cue that does not depend on the light.
//   8  flow direction. Red must stay black - it marks a gravity tangent pointing
//      upward, which is impossible. Green is how far the smoothed shading normal
//      had drifted from the facet, at 8x. Blue is a static band pattern across the
//      flow, so the direction can be read with no motion involved.
// Mode 2 says how often the probe is used at all; 3 and 4 say whether it is worth
// using, by showing what each source actually holds on this map. 3 and 4 are raw
// scene radiance and will clip bright - that is the point, they are being compared
// against each other, not viewed.
fn weather_surface_debug(input: VertexOut, weather: WeatherSurfaceResponse, mode: u32) -> vec3<f32> {
    if (mode == 8u) {
        // First, and before any non-uniform test, because it takes derivatives.
        let shading = normalize(input.world_normal);
        let facet = weather_facet_normal(input.world_position, shading);
        let gravity = vec3<f32>(0.0, -1.0, 0.0);
        let along = gravity - facet * dot(gravity, facet);
        let along_length = length(along);
        if (along_length < 0.05) {
            return vec3<f32>(0.0, 0.0, 0.15);
        }
        let flow = along / along_length;
        return vec3<f32>(
            // Must never light up: a gravity tangent cannot point upward.
            select(0.0, 1.0, flow.y > 0.001),
            // How far the smoothed shading normal had drifted from the facet, at 8x.
            clamp((1.0 - dot(facet, shading)) * 8.0, 0.0, 1.0),
            // Static bands across the flow - no time in it, so this is orientation
            // on its own, with none of the motion to argue about.
            fract(dot(input.world_position, flow) / WEATHER_RUNOFF_ALONG)
        );
    }
    if (mode == 6u) {
        return vec3<f32>(
            weather_material_porosity(),
            weather_material_gloss_response(),
            select(0.0, 1.0, (material.header.z & 4u) != 0u)
        );
    }
    if (mode == 7u) {
        let porosity = weather_material_porosity();
        let darkening = 0.34 * mix(0.65, 1.0, porosity) * clamp(weather.runoff, 0.0, 1.0);
        return vec3<f32>(clamp(darkening * 3.0, 0.0, 1.0));
    }
    if (mode == 5u) {
        return vec3<f32>(
            clamp(weather.runoff, 0.0, 1.0),
            0.0,
            clamp(abs(weather.runoff_tilt) * 6.0, 0.0, 1.0)
        );
    }
    if (mode == 1u) {
        let film_only = clamp(weather.wetness - weather.puddle * 0.65, 0.0, 1.0);
        let dry = vec3<f32>(0.05, 0.05, 0.05);
        let wet_film = vec3<f32>(0.12, 0.48, 0.14);
        let standing_water = vec3<f32>(0.05, 0.72, 1.0);
        return mix(mix(dry, wet_film, film_only), standing_water, weather.puddle);
    }
    if (weather.wetness <= 0.001 && weather.puddle <= 0.001) {
        return vec3<f32>(0.02);
    }
    let n = weather_reflection_normal(input.world_normal, weather);
    let v = normalize(camera.camera_pos_time.xyz - input.world_position);
    let reflection = normalize(reflect(-v, n));
    let environment = weather_coat_environment(reflection, weather.wetness, weather.puddle);
    if (mode == 2u) {
        // Deliberately not weighted by Fresnel: this is the mix, not the coat.
        return vec3<f32>(environment.sky_share, 1.0 - environment.sky_share, 0.0);
    }
    if (mode == 3u) {
        return environment.probe;
    }
    return environment.sky;
}

// Specular response of a wet surface to one local light. `wet_specular` is added
// on top of the diffuse lighting by the caller.
fn weather_light_specular(
    normal: vec3<f32>,
    view_direction: vec3<f32>,
    light_direction: vec3<f32>,
    weather: WeatherSurfaceResponse,
) -> f32 {
    let wet = clamp(weather.wetness, 0.0, 1.0);
    // Rivulets take the same water response as standing water, so a lamp on a wall
    // in the rain reads as light on running water and not as a damp sheen.
    let water_like = weather_water_like(weather);
    let amount = max(pow(wet, 1.35), water_like * 0.92)
        * mix(weather_material_gloss_response(), 1.0, water_like);
    if (weather_surface.look.z > 0.5) {
        // GGX with a floor on roughness: a mirror-flat puddle would otherwise
        // resolve a point light to a single aliasing pixel.
        return amount * 0.3 * water_specular(normal, view_direction, light_direction,
            mix(0.42, 0.18, water_like));
    }
    let half_vector = normalize(light_direction + view_direction);
    return amount
        * pow(max(dot(normal, half_vector), 0.0), mix(30.0, 140.0, max(wet, water_like)))
        * mix(0.18, 0.48, water_like);
}

// The sun reflected in wet ground: a broad sheen on wet film and a tight glint on
// rippled water. High quality only; shadowed exactly like the sun's own light.
fn weather_sun_glint(input: VertexOut, weather: WeatherSurfaceResponse) -> vec3<f32> {
    if (weather_surface.look.z < 0.5 || weather.wetness <= 0.001) {
        return vec3<f32>(0.0);
    }
    let sun = weather_surface.sun_radiance.rgb * weather_surface.sun_direction.w;
    if (max(sun.r, max(sun.g, sun.b)) <= 0.001) {
        return vec3<f32>(0.0);
    }
    let n = weather_reflection_normal(input.world_normal, weather);
    let v = normalize(camera.camera_pos_time.xyz - input.world_position);
    let l = normalize(-weather_surface.sun_direction.xyz);
    // A rivulet is water, so the sun answers it with the same tight lobe it gives a
    // puddle. On a wall this is the strongest cue there is: a narrow moving streak
    // of sunlight, where a damp wall only has a broad dim sheen.
    let water_like = weather_water_like(weather);
    let amount = max(pow(clamp(weather.wetness, 0.0, 1.0), 1.35), water_like)
        * weather_material_gloss_response();
    let glint = sun * (water_specular(n, v, l, mix(0.42, 0.16, water_like)) * amount * 0.5);
    // The lobe is vanishingly small away from the mirror direction: only pay for a
    // shadow lookup where there is a glint to shadow.
    if (max(glint.r, max(glint.g, glint.b)) < 0.002) {
        return vec3<f32>(0.0);
    }
    var visibility = 1.0;
    if (ENABLE_CASCADED_SHADOWS && shadow_settings.light_direction_enabled.w > 0.5) {
        visibility = cascaded_shadow_visibility(input, n);
    }
    return glint * visibility;
}

// Map-sky sampling for water environment lighting (the GodotOcean and wet ground).
// The CPU binds the map's primary skybox to every world surface, so water never
// falls back to a hardcoded blue environment. `weather_surface.sun_radiance.w` says
// whether the map really has one.
fn ocean_sky_uv(s: f32, t: f32) -> vec2<f32> {
    return clamp(vec2<f32>((s + 1.0) * 0.5, (1.0 - t) * 0.5), vec2<f32>(0.001), vec2<f32>(0.999));
}

fn sample_map_sky_renderer(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    // Convert renderer [x,z,-y] back to JKA [x,y,z].
    let d = normalize(vec3<f32>(direction.x, -direction.z, direction.y));
    let a = abs(d);
    // Sky textures carry their generated mip chain, so roughness can use it as
    // the cheap prefiltered-environment approximation this renderer already uses.
    let levels = max(textureNumLevels(sky_rt), 1u);
    let lod = clamp(roughness, 0.0, 1.0) * f32(levels - 1u);
    if (a.x >= a.y && a.x >= a.z) {
        if (d.x >= 0.0) {
            let m = a.x;
            return textureSampleLevel(sky_rt, sky_sampler, ocean_sky_uv(-d.y / m, d.z / m), lod).rgb;
        }
        let m = a.x;
        return textureSampleLevel(sky_lf, sky_sampler, ocean_sky_uv(d.y / m, d.z / m), lod).rgb;
    }
    if (a.y >= a.x && a.y >= a.z) {
        if (d.y >= 0.0) {
            let m = a.y;
            return textureSampleLevel(sky_bk, sky_sampler, ocean_sky_uv(d.x / m, d.z / m), lod).rgb;
        }
        let m = a.y;
        return textureSampleLevel(sky_ft, sky_sampler, ocean_sky_uv(-d.x / m, d.z / m), lod).rgb;
    }
    if (d.z >= 0.0) {
        let m = a.z;
        return textureSampleLevel(sky_up, sky_sampler, ocean_sky_uv(-d.y / m, -d.x / m), lod).rgb;
    }
    let m = a.z;
    return textureSampleLevel(sky_dn, sky_sampler, ocean_sky_uv(-d.y / m, d.x / m), lod).rgb;
}
