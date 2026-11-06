// The sky's shared uniforms and table bindings, at group 1 (group 0 is the renderer core's camera, which the
// sky never reads: the dome takes its view from these uniforms), read by the dome, the environment bake, the
// clouds and weather (`naval-clouds`) and diagnostics. The atmosphere is takram's Bruneton tables
// (`naval_sky::bruneton`, precomputed by `precompute.wgsl`), with the game's grade over them (`naval_sky::atmosphere`).
// `naval_sky::SkyUniforms` is the Rust side; keep the two in the same order. Every vec3 sits at a 16-byte offset
// and is followed by the scalar packed with it.
#define_import_path naval_sky::common

#import naval_sky::bruneton::{AtmosphereParameters}

struct SkyUniforms {
    // The dome's grade over the tables (`model::sky_grade`): game units in the dome per unit of the tables'
    // luminance for the sun (scale × twilight lift × gain, 0 once it has set) and the moon (its colour over the
    // sun's, × the grade's share), the saturation, and the authored gain on the higher scattering orders.
    sun_scale: vec3<f32>,
    saturation: f32,
    moon_scale: vec3<f32>,
    multiple: f32,
    // The aerosol phase's terms (`model::phase_terms`): the authored Cornette–Shanks lobe and the narrow core,
    // gain included, which the dome uses in place of the tables' single-Mie phase.
    phase_lobe: vec3<f32>,
    // The twilight exposure on everything the sun lights.
    sun_lift: f32,
    phase_core: vec3<f32>,
    // Camera altitude (km) the twilight lift was taken for.
    camera_height: f32,
    // Game units per unit of the tables for the light the clouds and the scene take: the sun's direct light
    // (relative luminance × scale × lift) and the sky's (× the dome's gain as well).
    direct_scale: vec3<f32>,
    // Zenith cosine of the camera's geometric horizon.
    camera_horizon_cosine: f32,
    diffuse_scale: vec3<f32>,
    // Game units per unit of relative luminance (`model::game_scale`: the sun's luminance above the air is the
    // scene's sun intensity).
    game_scale: f32,
    // The lights.
    sun_direction: vec3<f32>,
    sun_active: f32,
    sun_irradiance: vec3<f32>,
    moon_active: f32,
    moon_direction: vec3<f32>,
    moon_phase: f32,
    moon_irradiance: vec3<f32>,
    time: f32,
    // Camera world position (m).
    camera_position: vec3<f32>,
    // The sun's elevation (degrees) above the camera's horizon, as the twilight lift reads it.
    sun_elevation: f32,
    // The celestial bodies: rows of world -> galactic.
    galactic_x: vec3<f32>,
    night: f32,
    galactic_y: vec3<f32>,
    star_horizon: f32,
    galactic_z: vec3<f32>,
    star_limit: f32,
    moon_right: vec3<f32>,
    star_gain: f32,
    moon_up: vec3<f32>,
    milky_way: f32,
    moon_sun: vec3<f32>,
    moon_lit: f32,
    moon_surface: vec3<f32>,
    moon_opposition: f32,
    earthshine: vec3<f32>,
    moon_glow: f32,
    // The view: forward direction and radians per pixel at its centre.
    forward: vec3<f32>,
    pixel_angle: f32,
    // The active celestial light (for shafts and later parts).
    light_direction: vec3<f32>,
    bake_spread: f32,
    light_color: vec3<f32>,
    bake_level: f32,
    // The dome's view: clip -> view and view -> world (rotation only), and the viewport.
    inverse_projection: mat4x4<f32>,
    view_to_world: mat4x4<f32>,
    viewport: vec4<f32>,
    // For the clouds and weather (`naval-clouds`): the accumulated cloud drift (m, world XZ), lightning's scene
    // flash (a multiple of the ambient), the channel lighting the clouds and its radiant intensity (the web's
    // `SkyUniforms.windOffset`, `flash`, `lightningPosition`, `lightningIntensity`), and how the air under a
    // closed deck greys (x) and dims (y) the clear sky's haze (`overcast`).
    wind_offset: vec3<f32>,
    flash: f32,
    lightning_position: vec3<f32>,
    lightning_intensity: f32,
    haze: vec4<f32>,
    // The apparent size of the sun's (x) and the moon's (y) discs, multiples of DISC_RADIUS (`Sky::set_disc_scales`).
    discs: vec4<f32>,
}

@group(1) @binding(0) var<uniform> sky: SkyUniforms;
// takram's tables: transmittance (float32, read through a manual filter), the 4D scattering (Rayleigh and the
// higher orders, divided by the Rayleigh phase), single Mie, the higher orders alone, and the sky's irradiance.
@group(1) @binding(1) var transmittance_table: texture_2d<f32>;
@group(1) @binding(2) var scattering_table: texture_3d<f32>;
@group(1) @binding(3) var single_mie_table: texture_3d<f32>;
@group(1) @binding(4) var higher_order_table: texture_3d<f32>;
@group(1) @binding(5) var irradiance_table: texture_2d<f32>;
@group(1) @binding(6) var linear_clamp: sampler;
@group(1) @binding(7) var star_grid: texture_2d<f32>;
@group(1) @binding(8) var star_ramp: texture_2d<f32>;
@group(1) @binding(9) var milky_way_map: texture_2d<f32>;
@group(1) @binding(10) var milky_way_sampler: sampler;
// The air the tables were computed for (kilometres).
@group(1) @binding(11) var<uniform> air: AtmosphereParameters;

const PI: f32 = 3.141592653589793;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}
