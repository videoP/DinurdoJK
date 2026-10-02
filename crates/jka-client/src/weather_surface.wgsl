// Shared weather-surface math for the world shaders (bsp, bsp_lean), the SSR
// trace, the post pass and rain splashes. Pure functions plus the uniform layout:
// every consumer keeps its own bindings, so there is exactly one definition of
// where puddles are, how they are shaped and how their surface moves.
//
// The weather field is a map-wide RGBA32F grid (see weather/field.rs):
//   R  highest rain-blocking Y      G  highest top-facing surface Y
//   B  flat-area score (scatter)    A  basin score (major puddles)

struct WeatherSurfaceSettings {
    amount_distance: vec4<f32>, // wetness, grass fade start, grass fade end, rain-intensity response
    puddle: vec4<f32>,          // accumulation, time seconds, ripple strength, debug view
    occlusion_uv: vec4<f32>,    // min X/Z, inverse map width/depth
    occlusion_size: vec4<u32>,  // field width, height, active, reserved
    look: vec4<f32>,            // scattered puddle amount, reflection streak strength, high-quality water, film gloss
    film_fade: vec4<f32>,       // wet-film fade start, fade end (world units), reserved
    sun_direction: vec4<f32>,   // xyz light travel direction (render space), w relative intensity (250 = 1)
    sun_radiance: vec4<f32>,    // rgb linear sun colour, w: the map has a real skybox
    wind: vec4<f32>,            // render-space wind X/Z units per second, reserved
    // Footsteps, drags and landings (weather/wake.rs): count, lifetime, ring speed, now.
    wake_info: vec4<f32>,
    wake_a: array<vec4<f32>, 24>, // sole x, y, z, birth time
    wake_b: array<vec4<f32>, 24>, // strength, direction x, direction z, kind
};

const WEATHER_NO_COVER: f32 = -1.0e19;
const WEATHER_PI: f32 = 3.14159265359;

// ---------------------------------------------------------------- noise ----

fn weather_hash12(p: vec2<f32>) -> f32 {
    let p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    let q = p3 + vec3<f32>(dot(p3, p3.yzx + vec3<f32>(33.33)));
    return fract((q.x + q.y) * q.z);
}

fn weather_hash22(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        weather_hash12(p + vec2<f32>(17.17, 3.11)),
        weather_hash12(p + vec2<f32>(5.73, 41.91))
    );
}

fn weather_value_noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let u = f * f * f * (f * (f * 6.0 - vec2<f32>(15.0)) + vec2<f32>(10.0));
    let n00 = weather_hash12(cell);
    let n10 = weather_hash12(cell + vec2<f32>(1.0, 0.0));
    let n01 = weather_hash12(cell + vec2<f32>(0.0, 1.0));
    let n11 = weather_hash12(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(n00, n10, u.x), mix(n01, n11, u.x), u.y);
}

// Micro-relief of the ground: puddles form where the water level rises above it.
// Three rotated octaves keep blob edges from lining up with the noise grid.
// Mean 0.52, deviation 0.15; blobs are a few metres across at 40 units/metre.
fn weather_relief(world_xz: vec2<f32>) -> f32 {
    let rotation = mat2x2<f32>(0.8, 0.6, -0.6, 0.8);
    var p = world_xz / 210.0;
    let n0 = weather_value_noise(p);
    p = rotation * p * 2.37 + vec2<f32>(17.3, 5.1);
    let n1 = weather_value_noise(p);
    p = rotation * p * 2.41 + vec2<f32>(3.7, 11.9);
    let n2 = weather_value_noise(p);
    return 0.62 * n0 + 0.26 * n1 + 0.12 * n2;
}

// Broad 0..1 field that gathers puddles into low-lying regions.
fn weather_puddle_cluster(world_xz: vec2<f32>) -> f32 {
    return weather_value_noise(world_xz / 900.0 + vec2<f32>(31.7, 7.3));
}

// ---------------------------------------------------------------- field ----

struct WeatherField {
    exposure: f32,  // 1 open sky above the fragment, 0 fully covered
    flat_area: f32, // scattered-puddle suitability of the surface under the fragment
    basin: f32,     // enclosed-terrace score (major puddles)
};

fn weather_default_field() -> WeatherField {
    var field: WeatherField;
    field.exposure = 1.0;
    field.flat_area = 0.0;
    field.basin = 0.0;
    return field;
}

fn weather_exposure_from_cover(cover_y: f32, surface_y: f32) -> f32 {
    if (cover_y <= WEATHER_NO_COVER) {
        return 1.0;
    }
    return 1.0 - smoothstep(6.0, 24.0, cover_y - surface_y);
}

fn weather_field_load(field: texture_2d<f32>, texel: vec2<i32>) -> vec4<f32> {
    let dims = textureDimensions(field);
    let last = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    return textureLoad(field, clamp(texel, vec2<i32>(0), last), 0);
}

// Rain cover is one top-down height per texel, so on a slope the four texels
// around a fragment sit up to (texel size x tan(slope)) above or below it. Left
// alone, the uphill ones would read as "something above the fragment" and the
// wetness would flicker from full to less with every texel along the slope, as
// horizontal bands. Allow that much before a texel counts as cover. Near-vertical
// surfaces get none: a wall really is sheltered by its own top.
fn weather_slope_slack(normal_y: f32, texel_units: f32) -> f32 {
    let ny = clamp(normal_y, 0.0, 1.0);
    if (ny < 0.25) {
        return 0.0;
    }
    let tangent = sqrt(max(1.0 - ny * ny, 0.0)) / ny;
    return min(tangent * texel_units * 1.5, 40.0);
}

// Bilinear weather sample at a world position. Outside the mapped area the sky
// is open and nothing can pool. A texel only contributes to flat_area and basin
// when its top surface is at the fragment's own height, so a roof never lends its
// puddle to the floor beneath it. `normal_y` is the surface normal's up component
// (1 for level ground) and sets the slope allowance for the cover test.
fn weather_sample_field(
    field: texture_2d<f32>,
    world: vec3<f32>,
    min_xz: vec2<f32>,
    inverse_extent: vec2<f32>,
    normal_y: f32,
) -> WeatherField {
    var result = weather_default_field();
    let uv = (world.xz - min_xz) * inverse_extent;
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) {
        return result;
    }
    let dims = vec2<f32>(textureDimensions(field));
    let position = uv * dims - vec2<f32>(0.5);
    let base = vec2<i32>(floor(position));
    let blend = fract(position);

    let s00 = weather_field_load(field, base);
    let s10 = weather_field_load(field, base + vec2<i32>(1, 0));
    let s01 = weather_field_load(field, base + vec2<i32>(0, 1));
    let s11 = weather_field_load(field, base + vec2<i32>(1, 1));

    let cover_y = world.y + weather_slope_slack(normal_y, 1.0 / max(inverse_extent.x * dims.x, 1.0e-6));
    let e00 = weather_exposure_from_cover(s00.x, cover_y);
    let e10 = weather_exposure_from_cover(s10.x, cover_y);
    let e01 = weather_exposure_from_cover(s01.x, cover_y);
    let e11 = weather_exposure_from_cover(s11.x, cover_y);
    result.exposure = mix(mix(e00, e10, blend.x), mix(e01, e11, blend.x), blend.y);

    let m00 = 1.0 - smoothstep(6.0, 20.0, abs(s00.y - world.y));
    let m10 = 1.0 - smoothstep(6.0, 20.0, abs(s10.y - world.y));
    let m01 = 1.0 - smoothstep(6.0, 20.0, abs(s01.y - world.y));
    let m11 = 1.0 - smoothstep(6.0, 20.0, abs(s11.y - world.y));
    result.flat_area = mix(
        mix(s00.z * m00, s10.z * m10, blend.x),
        mix(s01.z * m01, s11.z * m11, blend.x),
        blend.y
    );
    result.basin = mix(
        mix(s00.w * m00, s10.w * m10, blend.x),
        mix(s01.w * m01, s11.w * m11, blend.x),
        blend.y
    );
    return result;
}

// -------------------------------------------------------------- puddles ----

struct WeatherPuddle {
    coverage: f32, // 1 under standing water, ~4 unit soft shoreline
    depth: f32,    // 0 at the shore, 1 in the deepest water
    damp: f32,     // wet margin: 1 at the shoreline, fading out ~20 units beyond it
};

// Standing water is "water level above ground relief". The level rises with rain
// accumulation and is raised much further inside enclosed basins, so light rain
// floods a basin in irregular pools that merge as the rain continues, and large
// flat ground collects scattered puddles clustered in its low regions. The same
// shape is evaluated everywhere puddles are drawn, so the material, the SSR
// trace, the post film and the rain splashes always agree.
fn weather_puddle_shape(
    field: WeatherField,
    world_xz: vec2<f32>,
    accumulation: f32,
    scatter_amount: f32,
) -> WeatherPuddle {
    var puddle: WeatherPuddle;
    puddle.coverage = 0.0;
    puddle.depth = 0.0;
    puddle.damp = 0.0;

    var level = 0.0;
    if (field.basin > 0.001) {
        level = field.basin * (0.14 + 1.6 * accumulation);
    }
    if (field.flat_area > 0.02 && scatter_amount > 0.01) {
        let eligibility = smoothstep(0.05, 0.6, field.flat_area);
        let amount = sqrt(smoothstep(0.0, 1.0, scatter_amount));
        let cluster = weather_puddle_cluster(world_xz);
        let scatter_level = amount * (0.235 + 0.15 * accumulation + (cluster - 0.5) * 0.14);
        level = max(level, scatter_level * eligibility);
    }
    if (level <= 0.001) {
        return puddle;
    }

    let submerged = level - weather_relief(world_xz);
    // A wide, soft shoreline: the water thins out over a metre or so, wetness
    // creeping out beyond it, rather than ending on a line.
    puddle.coverage = smoothstep(0.0, 0.22, submerged);
    puddle.depth = clamp(submerged / 0.35, 0.0, 1.0);
    puddle.damp = smoothstep(-0.18, 0.0, submerged);
    return puddle;
}

// -------------------------------------------------------------- ripples ----

// One expanding rain-drop ring per world cell. Behind the leading crest the
// surface rings for a couple of cycles and settles, like a real capillary packet.
// Returns the height gradient in the XZ plane.
fn weather_ripple_ring(
    world_xz: vec2<f32>,
    time_seconds: f32,
    uv_offset: vec2<f32>,
    phase_offset: f32,
    cell_size: f32,
) -> vec2<f32> {
    let uv = world_xz / cell_size + uv_offset;
    let cell = floor(uv);
    let local = fract(uv);
    let random = weather_hash22(cell + uv_offset * 19.0);
    // Centres stay in the middle of the cell and rings stop short of its border, so
    // a cell never slices a ring in half.
    let centre = vec2<f32>(0.30) + random * 0.40;
    let delta = local - centre;
    let dist = max(length(delta), 0.001);
    let age = fract(time_seconds * 0.52 + phase_offset
        + weather_hash12(cell + uv_offset * 31.0));
    let radius = mix(0.03, 0.27, age);
    let behind = radius - dist;
    let packet = smoothstep(-0.03, 0.02, behind) * (1.0 - smoothstep(0.10, 0.30, behind));
    let birth = smoothstep(0.0, 0.06, age);
    let fade = 1.0 - smoothstep(0.55, 1.0, age);
    return (delta / dist) * (cos(behind * 46.0) * packet * birth * fade);
}

// Wind-driven capillary waves travelling down-wind. Calm air leaves only the faint
// still-water shimmer; a fresh breeze streaks the puddle along the wind.
fn weather_capillary_gradient(
    world_xz: vec2<f32>,
    time_seconds: f32,
    wind: vec2<f32>,
    rain_strength: f32,
) -> vec2<f32> {
    let shimmer = vec2<f32>(
        sin(world_xz.x * 0.092 + world_xz.y * 0.037 + time_seconds * 2.7)
            + 0.55 * sin(world_xz.y * 0.121 - time_seconds * 3.2),
        cos(world_xz.y * 0.086 - world_xz.x * 0.031 - time_seconds * 2.9)
            + 0.50 * cos(world_xz.x * 0.115 + time_seconds * 3.5)
    ) * (0.07 * rain_strength);

    let speed = length(wind);
    let drive = clamp(speed / 500.0, 0.0, 1.0);
    if (drive <= 0.001) {
        return shimmer;
    }
    let direction = wind / speed;
    let along = dot(world_xz, direction);
    let across = dot(world_xz, vec2<f32>(-direction.y, direction.x));
    let phase = along * 0.16 - time_seconds * (2.0 + 5.0 * drive)
        + sin(across * 0.05 + time_seconds * 0.3) * 1.5;
    let chop = 0.6 * cos(phase)
        + 0.4 * cos(along * 0.23 - time_seconds * (3.2 + 7.0 * drive) + across * 0.11);
    return shimmer + direction * (chop * (0.03 + 0.20 * drive));
}

// Height gradient of the water surface. Rain intensity progressively enables one
// to four offset ring layers so heavy rain has a persistent overlapping field
// rather than one lonely disappearing ring.
fn weather_water_gradient(
    world_xz: vec2<f32>,
    time_seconds: f32,
    rain_strength: f32,
    wind: vec2<f32>,
) -> vec2<f32> {
    let weights = clamp(
        (vec4<f32>(rain_strength) - vec4<f32>(0.0, 0.25, 0.50, 0.75)) * 4.0,
        vec4<f32>(0.0),
        vec4<f32>(1.0)
    );
    var rings = vec2<f32>(0.0);
    if (weights.x > 0.0) {
        rings += weather_ripple_ring(world_xz, time_seconds, vec2<f32>( 0.25,  0.00), 0.00,  96.0) * weights.x;
    }
    if (weights.y > 0.0) {
        rings += weather_ripple_ring(world_xz, time_seconds, vec2<f32>(-0.55,  0.30), 0.31, 101.0) * weights.y;
    }
    if (weights.z > 0.0) {
        rings += weather_ripple_ring(world_xz, time_seconds, vec2<f32>( 0.60,  0.85), 0.57,  91.0) * weights.z;
    }
    if (weights.w > 0.0) {
        rings += weather_ripple_ring(world_xz, time_seconds, vec2<f32>( 0.50, -0.75), 0.79, 105.0) * weights.w;
    }
    return rings + weather_capillary_gradient(world_xz, time_seconds, wind, rain_strength);
}

// Unit water-surface normal (+Y up, render space) from a height gradient.
fn weather_water_normal(gradient: vec2<f32>, gain: f32) -> vec3<f32> {
    return normalize(vec3<f32>(-gradient.x * gain, 1.0, -gradient.y * gain));
}

// Footsteps ring at full size, a body dragging through the water makes small
// close-set ripples, a landing throws a big one.
fn weather_wake_kind_scale(kind: f32) -> f32 {
    if (kind > 1.5) {
        return 1.7;
    }
    if (kind > 0.5) {
        return 0.55;
    }
    return 1.0;
}

// ----------------------------------------------------------- water BRDF ----
// The GodotOcean water response (roughness-aware Fresnel, GGX sun glint) without
// its FFT/refraction machinery, so a puddle reads as the same liquid as the sea
// at a cost of a handful of ALU operations.

// Fresnel reflectance of a water film seen at cos_theta from the normal.
fn water_fresnel(cos_theta: f32, roughness: f32) -> f32 {
    let curve = pow(1.0 - clamp(cos_theta, 0.0, 1.0), 5.0 * exp(-2.69 * roughness))
        / (1.0 + 22.7 * pow(roughness, 1.5));
    return mix(curve, 1.0, 0.02);
}

fn water_ggx_distribution(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(WEATHER_PI * d * d, 1.0e-6);
}

// Height-correlated Smith visibility, already divided by 4 N.L N.V.
fn water_smith_visibility(n_dot_l: f32, n_dot_v: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1.0e-5);
}

// Specular response of a water surface to one directional light: D * Vis * F * N.L.
fn water_specular(
    normal: vec3<f32>,
    view_direction: vec3<f32>,
    light_direction: vec3<f32>,
    roughness: f32,
) -> f32 {
    let n_dot_l = max(dot(normal, light_direction), 0.0);
    if (n_dot_l <= 0.0) {
        return 0.0;
    }
    let n_dot_v = max(dot(normal, view_direction), 1.0e-3);
    let half_vector = normalize(light_direction + view_direction);
    let alpha = max(roughness * roughness, 0.002);
    // Plain Schlick (F0 = 2%) here: this runs per light, and the lobe's own width
    // already carries the roughness.
    let grazing = 1.0 - max(dot(half_vector, view_direction), 0.0);
    let grazing2 = grazing * grazing;
    let fresnel = 0.02 + 0.98 * grazing2 * grazing2 * grazing;
    return water_ggx_distribution(max(dot(normal, half_vector), 0.0), alpha)
        * water_smith_visibility(n_dot_l, n_dot_v, alpha)
        * fresnel * n_dot_l;
}
