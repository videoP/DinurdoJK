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
    water_normal: vec3<f32>, // ripple-perturbed water surface; straight up when dry
};

// Evaluated once per fragment: every later consumer (albedo, roughness, coat,
// specular, PBR) reads this instead of repeating the field fetch and ripple maths.
fn weather_surface_response(input: VertexOut) -> WeatherSurfaceResponse {
    var result: WeatherSurfaceResponse;
    result.wetness = 0.0;
    result.puddle = 0.0;
    result.depth = 0.0;
    result.rim = 0.0;
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

    var field = weather_default_field();
    if (weather_surface.occlusion_size.z != 0u) {
        field = weather_sample_field(
            weather_occlusion_height,
            world,
            weather_surface.occlusion_uv.xy,
            weather_surface.occlusion_uv.zw,
            normalize(input.world_normal).y
        );
    }

    let normal_y = normalize(input.world_normal).y;
    let orientation = smoothstep(-0.45, 0.12, normal_y);
    var wetness = clamp(weather_surface.amount_distance.x * weather_surface.amount_distance.w
        * distance_weight * field.exposure * orientation, 0.0, 1.0);

    let accumulation = clamp(weather_surface.puddle.x, 0.0, 1.0);
    if (accumulation > 0.001 && field.exposure > 0.001) {
        let shape = weather_puddle_shape(field, world.xz, accumulation, weather_surface.look.x);
        // Puddle placement is persistent world state: no camera-distance weight.
        // Walking toward or away from a pool must never create/remove it. A wall
        // that rises out of a flat floor is not part of the water.
        let upright = smoothstep(0.75, 0.95, normal_y);
        let coverage = shape.coverage * field.exposure * upright;
        result.puddle = coverage;
        result.depth = shape.depth;
        result.rim = shape.damp * (1.0 - coverage) * field.exposure * upright;
        // Standing water and its damp margin stay wet after the film has dried,
        // which lets puddles linger naturally after the rain stops.
        wetness = max(wetness, max(coverage * 0.96, shape.damp * field.exposure * upright * 0.85));
        if (coverage > 0.001) {
            // Ripples die away toward the shore instead of being cut by it.
            let gradient = (weather_water_gradient(
                world.xz,
                weather_surface.puddle.y,
                clamp(weather_surface.puddle.z, 0.0, 1.0),
                weather_surface.wind.xy
            ) + weather_wake_gradient(world)) * smoothstep(0.0, 0.5, shape.depth);
            result.water_normal = weather_water_normal(gradient, 0.12);
        }
    }
    result.wetness = wetness;
    return result;
}

// Water in a surface's pores darkens it (luminance^n, not a flat multiply): pale
// stone darkens a lot, near-black asphalt stays near-black, and the more water
// there is the stronger it gets. Only brightness changes: the surface keeps its
// own hue and saturation, so a wet wall never turns a colour of its own.
fn weather_wet_albedo(dry: vec3<f32>, weather: WeatherSurfaceResponse) -> vec3<f32> {
    let film = clamp(weather.wetness, 0.0, 1.0);
    let exponent = 1.0 + 0.45 * film + 0.45 * weather.puddle + 0.10 * weather.rim;
    let dry_luma = dot(dry, vec3<f32>(0.2126, 0.7152, 0.0722));
    if (dry_luma <= 1.0e-4) {
        return dry;
    }
    let wet_luma = min(pow(dry_luma, exponent), dry_luma);
    return mix(dry, dry * (wet_luma / dry_luma), film);
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

fn weather_wet_roughness(dry_roughness: f32, wetness: f32, puddle: f32) -> f32 {
    if (!ENABLE_WEATHER_SURFACE) {
        return clamp(dry_roughness, 0.01, 1.0);
    }
    let gloss_amount = pow(clamp(wetness, 0.0, 1.0), 1.35);
    let film_roughness = mix(dry_roughness, max(0.12, dry_roughness * 0.28),
        gloss_amount * weather_material_gloss_response());
    // Inside the wet mask the smoothness is overridden toward a true water-film
    // response instead of merely nudging the original material roughness.
    return clamp(mix(film_roughness, 0.012, smoothstep(0.05, 0.80, puddle)), 0.01, 1.0);
}

// Standing water is a new horizontal surface laid over the material, so it must
// not inherit the authored/smoothed material normal.
fn weather_reflection_normal(base_normal: vec3<f32>, weather: WeatherSurfaceResponse) -> vec3<f32> {
    let n = normalize(base_normal);
    if (!ENABLE_WEATHER_SURFACE || weather.puddle <= 0.001) {
        return n;
    }
    let water_mask = smoothstep(0.05, 0.85, weather.puddle);
    return normalize(mix(n, weather.water_normal, water_mask * 0.985));
}

fn weather_surface_coat(input: VertexOut, weather: WeatherSurfaceResponse) -> vec3<f32> {
    let wetness = weather.wetness;
    let puddle = weather.puddle;
    if (lighting_settings.feature_flags.w < 1u || (wetness <= 0.001 && puddle <= 0.001)) {
        return vec3<f32>(0.0);
    }
    let n = weather_reflection_normal(input.world_normal, weather);
    let v = normalize(camera.camera_pos_time.xyz - input.world_position);
    let n_dot_v = max(dot(n, v), 0.0);
    let reflection = normalize(reflect(-v, n));
    let jka_reflection = normalize(vec3<f32>(reflection.x, -reflection.z, reflection.y));
    let wet_lod = mix(0.25, 0.12, pow(clamp(wetness, 0.0, 1.0), 1.25));
    let lod_fraction = mix(wet_lod, 0.0, smoothstep(0.05, 0.80, puddle));
    var environment = textureSampleLevel(reflection_probe_texture, reflection_probe_sampler,
        jka_reflection, lod_fraction * f32(max(textureNumLevels(reflection_probe_texture), 1u) - 1u)).rgb;
    if (weather_surface.look.z > 0.5 && weather_surface.sun_radiance.w > 0.5) {
        // High quality water mirrors the map's own sky. Reflection quality gates the
        // probe cubemap and SSR, but the skybox is bound to every surface, so a wet
        // street or puddle can always show the real sky, fading out at the horizon
        // where the ground and buildings would be in the way.
        let sky = sample_map_sky_renderer(reflection, mix(0.30, 0.04, smoothstep(0.05, 0.80, puddle)));
        environment = mix(environment, sky, smoothstep(0.0, 0.16, reflection.y));
    }
    var fresnel = 0.025 + 0.975 * pow(1.0 - n_dot_v, 5.0);
    if (weather_surface.look.z > 0.5) {
        // High quality water uses the GodotOcean roughness-aware Fresnel.
        fresnel = water_fresnel(n_dot_v, min(weather_wet_roughness(0.65, wetness, puddle), 0.5));
    }
    let coat_amount = max(pow(clamp(wetness, 0.0, 1.0), 1.45), smoothstep(0.02, 0.75, puddle));
    let film_response = 0.34 * weather_material_gloss_response() * weather_surface.look.w;
    let water_response = 0.92;
    return environment * fresnel * coat_amount * mix(film_response, water_response, puddle);
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
    let puddle = weather.puddle;
    let amount = max(pow(wet, 1.35), puddle * 0.92)
        * mix(weather_material_gloss_response(), 1.0, puddle);
    if (weather_surface.look.z > 0.5) {
        // GGX with a floor on roughness: a mirror-flat puddle would otherwise
        // resolve a point light to a single aliasing pixel.
        return amount * 0.3 * water_specular(normal, view_direction, light_direction,
            mix(0.42, 0.18, puddle));
    }
    let half_vector = normalize(light_direction + view_direction);
    return amount
        * pow(max(dot(normal, half_vector), 0.0), mix(30.0, 140.0, max(wet, puddle)))
        * mix(0.18, 0.48, puddle);
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
    let amount = max(pow(clamp(weather.wetness, 0.0, 1.0), 1.35), weather.puddle)
        * weather_material_gloss_response();
    let glint = sun * (water_specular(n, v, l, mix(0.42, 0.16, weather.puddle)) * amount * 0.5);
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
