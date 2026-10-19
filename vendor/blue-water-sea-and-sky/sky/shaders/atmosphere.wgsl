// The sky's light as every part reads it: takram's atmosphere (Bruneton's precomputed scattering,
// `naval_sky::bruneton`, over the tables in the sky's group 1) in the game's units, with the game's grade over it
// (`SKY_GRADE`: a gain, the luminance shoulder, the chroma, the night floor; the twilight lift; the moon as a second
// light through the same tables; the authored aureole and multiple-scattering gain). Lengths are kilometres in the
// planet frame unless named otherwise: the planet's centre straight under the camera, world axes,
// `planet_km(p) = ((p.x - camera.x) / 1000, p.y / 1000 + 6360, (p.z - camera.z) / 1000)`.
//
// Units: the tables hold radiance for takram's solar spectrum; times takram's relative radiance-to-luminance
// factors, then the scene's scale (`model::game_scale`: the sun's luminance above the air is the scene's
// `sun_intensity`), radiance is in the sea's units, where a daylight sky pixel is about 0.3–1.0.
#define_import_path naval_sky::atmosphere

#import naval_sky::common::{sky, air, transmittance_table, scattering_table, single_mie_table, higher_order_table, irradiance_table, linear_clamp, luminance, PI}
#import naval_sky::bruneton::{Scattered, get_sky_radiance, get_sky_radiance_to_point, get_sun_and_sky_irradiance, get_sun_and_sky_scalar_irradiance, get_transmittance_to_top_atmosphere_boundary, get_transmittance_to_sun, rayleigh_phase_function, ray_intersects_ground}

// The bottom of the air (takram's) and its top: 100 km up, as the game's sky had it, rather than takram's 60 km, so
// the air over a camera in the blue hour (in the planet's shadow up to about 60 km with the sun 8° down) is lit.
const BOTTOM_RADIUS_KM: f32 = 6360.0;
const TOP_RADIUS_KM: f32 = 6460.0;
// The sea-level viewpoint of the environment bake (km above the sea).
const SEA_HEIGHT: f32 = 0.002;
// Half the width (cosine) over which the bodies fade into the horizon.
const HORIZON_EDGE: f32 = 0.0047;
// Below the horizon the dome holds the horizon's colour, read this far (cosine) above it.
const HORIZON_HOLD: f32 = 0.002;
// The sea's albedo under diffuse light, for the light the air below a camera returns.
const SEA_ALBEDO: f32 = 0.06;

// `SKY_GRADE` (model.rs).
const GRADE_KNEE: f32 = 0.45;
const GRADE_CEILING: f32 = 0.95;
const GRADE_FLOOR: vec3<f32> = vec3<f32>(0.006, 0.0105, 0.02);
const GRADE_FLOOR_HORIZON: f32 = 0.5;

// The contract with the clouds (`native/docs/sky.md`): irradiance from the sun and from the sky, game units.
struct SunSky {
    sun: vec3<f32>,
    skylight: vec3<f32>,
}

// Radiance (game units) and the transmittance along the same path.
struct SkyLight {
    radiance: vec3<f32>,
    transmittance: vec3<f32>,
}

// A world position (m) in the planet frame (km).
fn planet_km(position: vec3<f32>) -> vec3<f32> {
    let camera = sky.camera_position;
    return vec3<f32>(
        (position.x - camera.x) / 1000.0,
        position.y / 1000.0 + air.bottom_radius,
        (position.z - camera.z) / 1000.0,
    );
}

// The camera in the planet frame as the sky follows it (its altitude in 2% steps, `SkyUniforms.camera_height`):
// above the sea, never under it.
fn camera_km() -> vec3<f32> {
    return vec3<f32>(0.0, air.bottom_radius + max(sky.camera_height, SEA_HEIGHT), 0.0);
}

fn sea_km() -> vec3<f32> {
    return vec3<f32>(0.0, air.bottom_radius + SEA_HEIGHT, 0.0);
}

// The aerosol's phase times its gain: the authored Cornette–Shanks lobe plus the narrow forward core (the dome
// draws the aureole with it rather than the tables' plain Cornette–Shanks).
fn aerosol_phase(nu: f32) -> f32 {
    let lobe = sky.phase_lobe;
    let core = sky.phase_core;
    let x = inverseSqrt(max(lobe.y - lobe.z * nu, 1e-4));
    let y = inverseSqrt(max(core.y - core.z * nu, 1e-4));
    return lobe.x * (nu * nu + 1.0) * (x * x * x) + core.x * (y * y * y);
}

// takram reads all of the sky's radiance with its sky radiance-to-luminance factors, which assume a Rayleigh-like
// λ⁻³ spectrum. That holds for single Rayleigh scattering; light the aerosol scatters (single Mie, and in a hazy
// map's air most of the higher orders) keeps the sun's spectrum, so it takes the sun's factors (their ratio,
// sun / sky): a hazy map's haze then reads white, as its sunlit aureole does, rather than pink.
const SUN_TINT: vec3<f32> = vec3<f32>(0.8544714731, 0.9810456685, 1.0178296435);

// One light's scattered radiance in the tables' units (to be scaled by the sky's factors): single Rayleigh, the
// higher orders at the authored gain, and single Mie with the authored aureole.
fn lit(s: Scattered, nu: f32) -> vec3<f32> {
    return (s.rayleigh + s.multiple * (sky.multiple * SUN_TINT)) * rayleigh_phase_function(nu)
        + s.mie * aerosol_phase(nu) * SUN_TINT;
}

// The grade's shoulder and chroma over the scattered light (no floor).
fn shoulder(exposed: vec3<f32>) -> vec3<f32> {
    let brightness = max(luminance(exposed), 1e-6);
    let over = max(brightness - GRADE_KNEE, 0.0);
    let rolled = select(brightness, over / (over / (GRADE_CEILING - GRADE_KNEE) + 1.0) + GRADE_KNEE, brightness > GRADE_KNEE);
    return max(mix(vec3<f32>(brightness), exposed, sky.saturation), vec3<f32>(0.0)) * (rolled / brightness);
}

// The night floor (airglow and starlight) along a direction's vertical component.
fn night_floor(y: f32) -> vec3<f32> {
    let below = 1.0 - abs(y);
    let squared = below * below;
    return GRADE_FLOOR * (squared * squared * GRADE_FLOOR_HORIZON + 1.0);
}

// The dome's grade over scattered light, and the night floor.
fn grade(exposed: vec3<f32>, direction: vec3<f32>) -> vec3<f32> {
    return shoulder(exposed) + night_floor(direction.y);
}

// The sun's and the moon's scattered light along a ray from `origin` (tables' units times each light's scale),
// with `shadow_length` km of the sun's shafts; the phases are taken toward `phase_direction` (the dome holds the
// horizon's colour below it, but its aureole fades below the horizon as the ray turns from the light).
fn scattered_along(
    origin: vec3<f32>,
    direction: vec3<f32>,
    phase_direction: vec3<f32>,
    shadow_length: f32,
) -> vec3<f32> {
    var light = vec3<f32>(0.0);
    if sky.sun_active > 0.5 {
        let s = get_sky_radiance(
            air, transmittance_table, scattering_table, single_mie_table, higher_order_table, linear_clamp,
            origin, direction, shadow_length, sky.sun_direction,
        );
        light += lit(s, dot(phase_direction, sky.sun_direction)) * sky.sun_scale;
    }
    if sky.moon_active > 0.5 {
        let s = get_sky_radiance(
            air, transmittance_table, scattering_table, single_mie_table, higher_order_table, linear_clamp,
            origin, direction, 0.0, sky.moon_direction,
        );
        light += lit(s, dot(phase_direction, sky.moon_direction)) * sky.moon_scale;
    }
    return light;
}

// Zenith cosine of the geometric horizon from radius `r` (km).
fn horizon_cosine(r: f32) -> f32 {
    let ratio = air.bottom_radius / r;
    return -sqrt(max(1.0 - ratio * ratio, 0.0));
}

// Below the horizon the dome holds the horizon's colour: the direction lifted to just above it.
fn held(direction: vec3<f32>, r: f32) -> vec3<f32> {
    let lowest = horizon_cosine(r) + HORIZON_HOLD;
    if direction.y >= lowest {
        return direction;
    }
    let across = direction.xz * inverseSqrt(max(dot(direction.xz, direction.xz), 1e-12));
    return vec3<f32>(across.x * sqrt(1.0 - lowest * lowest), lowest, across.y * sqrt(1.0 - lowest * lowest));
}

// --- the contract with the clouds ----------------------------------------------------------------------------

// takram's GetSunAndSkyScalarIrradiance for the sun: its direct light through the air and the planet's shadow
// (no cosine), and the sky's over the whole sphere; game units, the twilight lift included, the night floor in the
// sky's.
fn sun_sky_scalar_irradiance(point_km: vec3<f32>) -> SunSky {
    let s = get_sun_and_sky_scalar_irradiance(
        air, transmittance_table, irradiance_table, linear_clamp, point_km, sky.sun_direction,
    );
    return SunSky(s.sun * sky.direct_scale, s.skylight * sky.diffuse_scale + GRADE_FLOOR * (2.0 * PI));
}

// takram's GetSunAndSkyIrradiance on a surface facing `normal`, game units.
fn sun_sky_irradiance(point_km: vec3<f32>, normal: vec3<f32>) -> SunSky {
    let s = get_sun_and_sky_irradiance(
        air, transmittance_table, irradiance_table, linear_clamp, point_km, normal, sky.sun_direction,
    );
    let up = dot(normal, normalize(point_km)) * 0.5 + 0.5;
    return SunSky(s.sun * sky.direct_scale, s.skylight * sky.diffuse_scale + GRADE_FLOOR * (PI * up));
}

// GetSkyRadianceToPoint with `shadow_length_km` of light shafts: the light the air between the two points scatters
// toward the camera, as the dome shows the sky (the sun, the moon at night, the grade, the night floor in proportion
// to the air's opacity), so distant clouds fade into the dome's horizon; and the transmittance between them.
fn sky_radiance_to_point(camera: vec3<f32>, point: vec3<f32>, shadow_length_km: f32) -> SkyLight {
    let direction = normalize(point - camera);
    var light = vec3<f32>(0.0);
    var transmittance = vec3<f32>(1.0);
    if sky.sun_active > 0.5 {
        let s = get_sky_radiance_to_point(
            air, transmittance_table, scattering_table, single_mie_table, higher_order_table, linear_clamp,
            camera, point, shadow_length_km, sky.sun_direction,
        );
        light += lit(s, dot(direction, sky.sun_direction)) * sky.sun_scale;
        transmittance = s.transmittance;
    }
    if sky.moon_active > 0.5 {
        let s = get_sky_radiance_to_point(
            air, transmittance_table, scattering_table, single_mie_table, higher_order_table, linear_clamp,
            camera, point, 0.0, sky.moon_direction,
        );
        light += lit(s, dot(direction, sky.moon_direction)) * sky.moon_scale;
        transmittance = s.transmittance;
    }
    if sky.sun_active <= 0.5 && sky.moon_active <= 0.5 {
        transmittance = get_sky_radiance_to_point(
            air, transmittance_table, scattering_table, single_mie_table, higher_order_table, linear_clamp,
            camera, point, 0.0, sky.sun_direction,
        ).transmittance;
    }
    let opacity = 1.0 - clamp(luminance(transmittance), 0.0, 1.0);
    return SkyLight(shoulder(light) + night_floor(direction.y) * opacity, transmittance);
}

// GetSkyRadiance along a ray from `camera` (km), with `shadow_length_km` of the sun's shafts: the graded sky with
// its night floor (no discs or stars), and the transmittance to space (0 toward the ground).
fn sky_radiance_along(camera: vec3<f32>, direction: vec3<f32>, shadow_length_km: f32) -> SkyLight {
    let r = length(camera);
    let mu = dot(camera, direction) / r;
    let ground = ray_intersects_ground(air, r, mu);
    let transmittance = select(
        get_transmittance_to_top_atmosphere_boundary(air, transmittance_table, min(r, air.top_radius), mu),
        vec3<f32>(0.0),
        ground,
    );
    let up = camera / r;
    let local = vec3<f32>(direction.x, dot(direction, up), direction.z);
    return SkyLight(grade(scattered_along(camera, direction, direction, shadow_length_km), local), transmittance);
}

// --- the sky's own reads, and the old interface --------------------------------------------------------------

// The dome's sky from the camera (or, with `from_sea`, from sea level under it, the environment bake's viewpoint)
// with `shadow_length` km of the sun's shafts: below the horizon, the horizon's colour.
fn dome_sky(direction: vec3<f32>, from_sea: bool, shadow_length: f32) -> vec3<f32> {
    let origin = select(camera_km(), sea_km(), from_sea);
    let along = held(direction, origin.y);
    // The host's night sky brightness (`sky.haze.z`) dims the dome's own night floor; the environment bake (from the
    // sea) keeps the full floor so the world's night light is unchanged.
    let night = select(sky.haze.z, 1.0, from_sea);
    return shoulder(scattered_along(origin, along, direction, shadow_length)) + night_floor(direction.y) * night;
}

// Sky radiance along a world direction, sun and moon discs excluded; below the horizon, the horizon's colour.
// Seen from the camera, or with `from_sea` from sea level under it (the environment bake).
fn sky_radiance(direction: vec3<f32>, from_sea: bool) -> vec3<f32> {
    return dome_sky(direction, from_sea, 0.0);
}

// The clear sky's radiance `sky_color` toward the horizon as the air under the cloud layer shows it: greyed and
// dimmed under a closed deck (`overcast`), unchanged otherwise.
fn overcast(sky_color: vec3<f32>) -> vec3<f32> {
    return mix(sky_color, vec3<f32>(luminance(sky_color)), sky.haze.x) * sky.haze.y;
}

// The far fog's colour along a world direction: the sky from the camera without discs, stars or clouds, greyed
// under a closed deck (`OceanSky.createFogSampler`, `Sky.ts`'s `fog` source).
fn fog_radiance(direction: vec3<f32>) -> vec3<f32> {
    return overcast(sky_radiance(direction, false));
}

// Transmittance to space along a direction, from the camera or from sea level, faded into the horizon.
fn transmittance_to_space(direction: vec3<f32>, from_sea: bool) -> vec3<f32> {
    let origin = select(camera_km(), sea_km(), from_sea);
    let r = origin.y;
    let mu = direction.y;
    let horizon = horizon_cosine(r);
    let above = vec3<f32>(direction.x, max(mu, horizon + HORIZON_EDGE), direction.z);
    return get_transmittance_to_top_atmosphere_boundary(air, transmittance_table, r, above.y)
        * smoothstep(horizon - HORIZON_EDGE, horizon + HORIZON_EDGE, mu);
}

// Share of the sun's light reaching a world position (m), with the sky's twilight exposure (`sunTransmittance`).
fn sun_transmittance(position: vec3<f32>) -> vec3<f32> {
    let point = planet_km(position);
    let r = max(length(point), air.bottom_radius);
    return get_transmittance_to_sun(air, transmittance_table, r, dot(point, sky.sun_direction) / length(point)) * sky.sun_lift;
}

fn moon_transmittance(position: vec3<f32>) -> vec3<f32> {
    let point = planet_km(position);
    let r = max(length(point), air.bottom_radius);
    return get_transmittance_to_sun(air, transmittance_table, r, dot(point, sky.moon_direction) / length(point));
}

struct Aerial {
    inscatter: vec3<f32>,
    transmittance: vec3<f32>,
}

// Aerial perspective between the viewpoint and the point `distance` metres along `direction`.
fn aerial(direction: vec3<f32>, distance: f32, from_sea: bool) -> Aerial {
    let origin = select(camera_km(), sea_km(), from_sea);
    let light = sky_radiance_to_point(origin, origin + direction * max(distance / 1000.0, 1e-4), 0.0);
    return Aerial(light.radiance, light.transmittance);
}

struct Ambient {
    above: vec3<f32>,
    below: vec3<f32>,
}

// The sky's mean radiance over the hemisphere above an altitude (m), and that of the air and sea below it (the
// sea's albedo under the sun and sky reaching it, through the air between).
fn ambient(altitude: f32) -> Ambient {
    let r = air.bottom_radius + max(altitude / 1000.0, SEA_HEIGHT);
    let point = vec3<f32>(0.0, r, 0.0);
    let up = vec3<f32>(0.0, 1.0, 0.0);
    var above = vec3<f32>(0.0);
    var sea = vec3<f32>(0.0);
    let at_sea = vec3<f32>(0.0, air.bottom_radius + SEA_HEIGHT, 0.0);
    if sky.sun_active > 0.5 {
        let here = get_sun_and_sky_irradiance(air, transmittance_table, irradiance_table, linear_clamp, point, up, sky.sun_direction);
        above += here.skylight * sky.diffuse_scale;
        let low = get_sun_and_sky_irradiance(air, transmittance_table, irradiance_table, linear_clamp, at_sea, up, sky.sun_direction);
        sea += low.sun * sky.direct_scale + low.skylight * sky.diffuse_scale;
    }
    if sky.moon_active > 0.5 {
        // The moon through the same tables, at the dome's share of it.
        let here = get_sun_and_sky_irradiance(air, transmittance_table, irradiance_table, linear_clamp, point, up, sky.moon_direction);
        let low = get_sun_and_sky_irradiance(air, transmittance_table, irradiance_table, linear_clamp, at_sea, up, sky.moon_direction);
        above += here.skylight * sky.moon_scale;
        sea += (low.sun + low.skylight) * sky.moon_scale;
    }
    let down = get_transmittance_to_top_atmosphere_boundary(air, transmittance_table, r, 0.0);
    return Ambient(
        above / PI + GRADE_FLOOR,
        sea * (SEA_ALBEDO / PI) * mix(vec3<f32>(1.0), down, 0.5) + GRADE_FLOOR * 0.5,
    );
}
