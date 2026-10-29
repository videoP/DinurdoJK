// The clouds' uniforms, their group-2 bindings and small maths every cloud pass shares. A port of the uniform set of
// @takram/three-clouds (`uniforms.ts`, `parameters.glsl`, `CloudsMaterial.ts`, `ShadowMaterial.ts`) packed into
// vec4s; `naval_clouds::CloudUniforms` is the Rust side, field for field. Each pass binds only the group-2 entries
// it reads: the field (0–5, 7, 8) everywhere, the Beer shadow maps (6) in the march, the light-shaft histories
// (9, 10) in the sky's dome, the scene's depth (11) in the march.
//
// Frames: world metres with +Y up. Positions inside the cloud passes are in the PLANET frame, takram's ECEF stand-in:
// the planet's centre straight under the camera, world axes, so a point's height is |p| − planet radius and the sea
// curves away from the camera as the atmosphere does. The textures are looked up in the clouds' drifting world frame
// (`naval_clouds::field::noise_position`), never in the planet frame, which moves with the camera.
//
// MIT License, Copyright (c) 2024 Shota Matsuda (https://github.com/takram-design-engineering/three-geospatial). The
// full notice is in `naval-clouds/THIRD_PARTY.md`.
#define_import_path naval_clouds::common

struct CloudUniforms {
    // World → view (the cascades' depth and, by its rotation, the march's rays) and the previous update's world → clip
    // (reprojection).
    view: mat4x4<f32>,
    previous_view_projection: mat4x4<f32>,
    // xyz the camera (world m), w its height above the sea (m).
    camera: vec4<f32>,
    // xyz the camera's forward (world), w 1 / tan(half the vertical field of view).
    forward: vec4<f32>,
    // xy the cloud history/reconstruction buffer (px), zw its quarter-size march buffer.
    screen: vec4<f32>,
    // xy the full output viewport (px). Depth/probe inputs and dome fragments use this resolution.
    viewport: vec4<f32>,
    // x the update counter, yz this update's cell of the 4 × 4 Bayer block (x right, y down), w 1 when the
    // reconstruction may read its history.
    frame: vec4<f32>,
    // x near plane, y far plane, z the shadow cascades' far, w the weather's mip scale (takram's `mipLevelScale`).
    planes: vec4<f32>,
    // xyz toward the sun (world), w the planet's radius (m).
    sun: vec4<f32>,
    // xyz the clouds' drift (m, world), w the shape's evolution (m, takram's `localWeatherSpeed × 2e4`).
    drift: vec4<f32>,
    // x scattering coefficient, y absorption coefficient, z coverage, w turbulence displacement (m).
    medium: vec4<f32>,
    // x weather tiles per metre, y shape repeat (per m), z detail repeat (per m), w turbulence tiles per weather tile.
    repeats: vec4<f32>,
    // The four layers, one per component.
    min_layer_heights: vec4<f32>,
    max_layer_heights: vec4<f32>,
    // The gaps between layers (three, in xyz) the march strides over.
    min_interval_heights: vec4<f32>,
    max_interval_heights: vec4<f32>,
    density_scales: vec4<f32>,
    shape_amounts: vec4<f32>,
    shape_detail_amounts: vec4<f32>,
    weather_exponents: vec4<f32>,
    shape_altering_biases: vec4<f32>,
    coverage_filter_widths: vec4<f32>,
    // x lowest layer bottom, y highest layer top, z top of the shadow-casting layers, w their bottom (m).
    heights: vec4<f32>,
    // 1 for the layers that cast shadows.
    shadow_layer_mask: vec4<f32>,
    // The layers' density profiles: exp_term · exp(exponent · h) + linear_term · h + constant_term.
    density_exp_terms: vec4<f32>,
    density_exponents: vec4<f32>,
    density_linear_terms: vec4<f32>,
    density_constant_terms: vec4<f32>,
    // Column i picks layer i's channel of the weather texture.
    channels: mat4x4<f32>,
    // x sky light scale, y ground bounce scale, z powder scale, w powder exponent (the powder on slabs only).
    scattering: vec4<f32>,
    // x first anisotropy, y second anisotropy, z their mix, w broken cloud's multiple-scattering octaves (four at most).
    phase: vec4<f32>,
    // The primary march: x most iterations, y least step (m), z longest step (m), w longest ray (m).
    march: vec4<f32>,
    // x perspective step scale, y least density, z least extinction, w least transmittance.
    march_limits: vec4<f32>,
    // The secondary marches: x iterations toward the sun, y toward the ground, z least step (m), w step scale.
    secondary: vec4<f32>,
    // Light shafts: x most iterations, y least step (m), z longest ray (m), w 1 when on.
    shafts: vec4<f32>,
    // Haze: x density scale, y exponent, z scattering coefficient, w absorption coefficient.
    haze: vec4<f32>,
    // 1 or 0: x shape detail, y turbulence, z haze, w accurate sun and sky light.
    switches: vec4<f32>,
    // 1 or 0: x ground bounce, y powder, z clouds drawn, w the light-shaft history this frame's reconstruction writes.
    state: vec4<f32>,
    // Beer shadow maps: x cascades, y texels per edge, z most filter radius (texels), w filter samples.
    shadow_map: vec4<f32>,
    // The shadow march: x most iterations, y least step (m), z longest step (m), w least density.
    shadow_march: vec4<f32>,
    // x least extinction, y least transmittance, z optical depth tail scale, w 1 when the history may be read.
    shadow_limits: vec4<f32>,
    // xy: each cascade's interval, in fractions of the cascades' far.
    shadow_intervals: array<vec4<f32>, 4>,
    // World → each cascade's clip; its inverse; and the previous update's world → clip.
    shadow_matrices: array<mat4x4<f32>, 4>,
    inverse_shadow_matrices: array<mat4x4<f32>, 4>,
    shadow_reprojection: array<mat4x4<f32>, 4>,
    // The ground shadow map: x texels per edge, y first texel of this update's slice, z texels per slice, w steps.
    ground: vec4<f32>,
    // x the environment bake's steps, y the ground shadow's strength (0 with its body low), z and w unused.
    bake: vec4<f32>,
    // x the cascades the shadow pass marches this update, a bit each (all after a cut, else the round robin's and any the
    // view has left), y how many.
    cascades: vec4<f32>,
    // xyz toward the body whose light the ground shadow map stops: the sun, or the moon once it outshines the sun. w
    // unused.
    ground_light: vec4<f32>,
    // Each layer's coverage (takram's one coverage, `medium.z`, for all four): the weather's layers.
    layer_coverages: vec4<f32>,
    // 1 for the layers that are closed slabs (decks, fog): the light under them diffuses down through them.
    layer_slabs: vec4<f32>,
    // xyz how far the wind carried the clouds since the previous update (m, world; 0 without history). w unused.
    drift_step: vec4<f32>,
    // The clouds' key light (`naval_clouds::key_light`, `sun` points at it): xyz the moon's direct scale (game units per
    // unit of the tables' solar irradiance), w 1 while the moon is the key light.
    key_light: vec4<f32>,
    // x 1 while a lightning flash's light on the clouds changes (it, or the last update's, lit them): the reconstruction
    // clips its history to the march as a moving view's does, so the flash shows and goes with the march. yzw unused.
    flash: vec4<f32>,
    // x debug view, y bypass Beer attenuation, z far-ray step multiplier, w use mean local sun depth.
    experiments: vec4<f32>,
}

@group(2) @binding(0) var<uniform> clouds: CloudUniforms;
// takram's local weather (512², a channel per layer, mipmapped) and shape volume (128³), the crate's detail noise
// (128³ with mips, four channels: `naval_clouds::detail_noise`) and the turbulence (128²).
@group(2) @binding(1) var local_weather: texture_2d<f32>;
@group(2) @binding(2) var shape_volume: texture_3d<f32>;
@group(2) @binding(3) var detail_volume: texture_3d<f32>;
@group(2) @binding(4) var turbulence_map: texture_2d<f32>;
// The clouds' own 128² blue noise (`examples/blue_noise.rs`), animated over frames by golden-ratio offsets.
@group(2) @binding(5) var blue_noise: texture_2d<f32>;
// The Beer shadow maps, one layer per cascade: front depth (km), mean extinction, most optical depth, its tail.
@group(2) @binding(6) var shadow_maps: texture_2d_array<f32>;
@group(2) @binding(7) var repeat_sampler: sampler;
@group(2) @binding(8) var clamp_sampler: sampler;
// The reconstructed light-shaft lengths (km, r), the two histories the reconstruction alternates between.
@group(2) @binding(9) var shadow_length_a: texture_2d<f32>;
@group(2) @binding(10) var shadow_length_b: texture_2d<f32>;
// The opaque scene's reversed depth (`scene.opaque.depth`).
@group(2) @binding(11) var scene_depth: texture_depth_2d;

const PI: f32 = 3.141592653589793;
const PI2: f32 = 6.283185307179586;
const RECIPROCAL_PI: f32 = 0.3183098861837907;
const RECIPROCAL_PI4: f32 = 0.07957747154594767;
const METER_TO_KM: f32 = 0.001;

fn saturate4(x: vec4<f32>) -> vec4<f32> {
    return clamp(x, vec4<f32>(0.0), vec4<f32>(1.0));
}

// takram's `remapClamped(x, min1, max1)`: (x − min1) / (max1 − min1) clamped to [0, 1]. An empty range (an unused
// layer's, a zero filter width) counts as 1e-7 wide instead of dividing by zero.
fn remap_clamped(x: f32, min1: f32, max1: f32) -> f32 {
    let width = max1 - min1;
    return clamp((x - min1) / select(width, 1e-7, abs(width) < 1e-7), 0.0, 1.0);
}

fn remap_clamped4(x: vec4<f32>, min1: vec4<f32>, max1: vec4<f32>) -> vec4<f32> {
    let width = max1 - min1;
    return saturate4((x - min1) / select(width, vec4<f32>(1e-7), abs(width) < vec4<f32>(1e-7)));
}

// takram's `remap(x, min1, max1, min2, max2)`.
fn remap(x: f32, min1: f32, max1: f32, min2: f32, max2: f32) -> f32 {
    return min2 + (x - min1) / (max1 - min1) * (max2 - min2);
}

// Interleaved gradient noise (Jimenez 2014).
fn interleaved_gradient_noise(coord: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(coord, vec2<f32>(0.06711056, 0.00583715))));
}

// A world position (m) in the planet frame (m).
fn world_to_planet(world: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(world.x - clouds.camera.x, world.y + clouds.sun.w, world.z - clouds.camera.z);
}

// A planet-frame position (m) in the world (m).
fn planet_to_world(planet: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(planet.x + clouds.camera.x, planet.y - clouds.sun.w, planet.z + clouds.camera.z);
}

// The camera in the planet frame (m).
fn camera_planet() -> vec3<f32> {
    return vec3<f32>(0.0, clouds.sun.w + clouds.camera.w, 0.0);
}

// Whether the view ray through screen point `uv` (0–1, y down) starts in the water: `probe` is the ocean's underwater
// probe (`naval_ocean::underwater`: each pixel's near-plane point's depth below the waves and the wake, positive under,
// never positive while the near plane cannot reach the water) or the layer's own 1 × 1 zero without an ocean. The sea's
// surface then ends the ray before any cloud, haze or light shaft; the sea's underside shows the sky, clouds and all,
// through Snell's window from the environment, and the underwater view fogs the rest.
fn ray_starts_under_water(probe: texture_2d<f32>, uv: vec2<f32>) -> bool {
    return textureSampleLevel(probe, clamp_sampler, uv, 0.0).r > 0.0;
}

// The far intersection of a ray with a sphere about the planet's centre, or −1 when it misses.
fn ray_sphere_second(origin: vec3<f32>, direction: vec3<f32>, radius: f32) -> f32 {
    let b = 2.0 * dot(direction, origin);
    let c = dot(origin, origin) - radius * radius;
    let discriminant = b * b - 4.0 * c;
    return select((-b + sqrt(max(discriminant, 0.0))) * 0.5, -1.0, discriminant < 0.0);
}
