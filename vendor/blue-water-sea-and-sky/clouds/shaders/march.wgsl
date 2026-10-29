// The clouds' ray marches and lighting, a port of `clouds.frag` of @takram/three-clouds: the primary march with
// Frostbite's energy-conserving integration, the secondary marches toward the sun and the ground, the Beer shadow maps
// with their cascades and PCF, the dual-lobe phase, the ground bounce, light shafts (shadow length), haze and the aerial
// perspective on the clouds. The light of broken cloud departs from takram's (2026-10-04, `sample_radiance`): three
// octaves of multiple scattering in place of eight, a narrow forward lobe on the first, an edge term by the view ray's
// depth in the cloud in place of powder, and the sky's light less what the cloud about a sample hides of it; slabs
// (decks, fog) keep takram's octaves and powder. Positions are in the planet frame (m); the atmosphere's functions take
// kilometres.
//
// MIT License, Copyright (c) 2024 Shota Matsuda (https://github.com/takram-design-engineering/three-geospatial). The
// full notice is in `naval-clouds/THIRD_PARTY.md`. The cascade selection follows three.js's CSM shader (MIT).
#define_import_path naval_clouds::march

#import naval_clouds::common::{clouds, shadow_maps, clamp_sampler, remap_clamped, interleaved_gradient_noise, planet_to_world, world_to_planet, ray_sphere_second, saturate4, PI, PI2, RECIPROCAL_PI, RECIPROCAL_PI4, METER_TO_KM}
#import naval_clouds::field::{WeatherSample, MediaSample, ColumnAbove, sample_weather, sample_media, sample_media_at, detail_lod, weather_uv, inside_layer_intervals, layer_density, column_above, any_deck}
#import naval_sky::atmosphere::{SunSky, SkyLight, sun_sky_scalar_irradiance, sun_sky_irradiance, sky_radiance_to_point, GRADE_FLOOR}
#import naval_sky::common::{sky, air, transmittance_table, irradiance_table, linear_clamp}
#import naval_sky::bruneton::{get_sun_and_sky_scalar_irradiance, get_sun_and_sky_irradiance}

// ---- The key light ----------------------------------------------------------------------------------------------

// The light on the clouds at a planet-frame point (km): takram's sun and sky (`sun_sky_scalar_irradiance`) while the
// sun is the key light. Once the moon is (`clouds.key_light`, `naval_clouds::key_light`: its direct light at the top of
// the clouds outshining the sun's), its beam through the same tables at the world's moonlight, and the sky's light: the
// moonlit sky at the dome's grade (`sky.moon_scale`), the sun's twilight while it still lights the sky, the night floor.
fn key_scalar_irradiance(point_km: vec3<f32>) -> SunSky {
    if clouds.key_light.w < 0.5 {
        return sun_sky_scalar_irradiance(point_km);
    }
    let moon = get_sun_and_sky_scalar_irradiance(air, transmittance_table, irradiance_table, linear_clamp, point_km, sky.moon_direction);
    var skylight = moon.skylight * sky.moon_scale + GRADE_FLOOR * PI2;
    if sky.sun_active > 0.5 {
        skylight += get_sun_and_sky_scalar_irradiance(air, transmittance_table, irradiance_table, linear_clamp, point_km, sky.sun_direction).skylight * sky.diffuse_scale;
    }
    return SunSky(moon.sun * clouds.key_light.xyz, skylight);
}

// The same on a surface facing `normal` (`sun_sky_irradiance`): the ground bounce's light.
fn key_irradiance(point_km: vec3<f32>, normal: vec3<f32>) -> SunSky {
    if clouds.key_light.w < 0.5 {
        return sun_sky_irradiance(point_km, normal);
    }
    let moon = get_sun_and_sky_irradiance(air, transmittance_table, irradiance_table, linear_clamp, point_km, normal, sky.moon_direction);
    let up = dot(normal, normalize(point_km)) * 0.5 + 0.5;
    var skylight = moon.skylight * sky.moon_scale + GRADE_FLOOR * (PI * up);
    if sky.sun_active > 0.5 {
        skylight += get_sun_and_sky_irradiance(air, transmittance_table, irradiance_table, linear_clamp, point_km, normal, sky.sun_direction).skylight * sky.diffuse_scale;
    }
    return SunSky(moon.sun * clouds.key_light.xyz, skylight);
}

// ---- Beer shadow maps -------------------------------------------------------------------------------------------

// A world position's texture coordinates in a cascade (y down, as the shadow pass lays its texels out).
fn shadow_uv(world: vec3<f32>, cascade: i32) -> vec2<f32> {
    let clip = clouds.shadow_matrices[cascade] * vec4<f32>(world, 1.0);
    let ndc = clip.xy / clip.w;
    return vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
}

// The distance from a point toward the sun to the top of the shadow-casting layers: where the shadow march started.
fn distance_to_shadow_top(position: vec3<f32>) -> f32 {
    return ray_sphere_second(position, clouds.sun.xyz, clouds.sun.w + clouds.heights.z);
}

// A point's depth along the view in fractions of the cascades' far (three's `viewZToOrthographicDepth`).
fn cascade_depth(world: vec3<f32>) -> f32 {
    let view = clouds.view * vec4<f32>(world, 1.0);
    let near = clouds.planes.x;
    let far = clouds.planes.z;
    return (view.z + near) / (near - far);
}

// The cascade a point reads, fading into the next one over a margin by the jitter (`getFadedCascadeIndex`); −1 for
// none.
fn faded_cascade_index(world: vec3<f32>, jitter: f32) -> i32 {
    let depth = cascade_depth(world);
    let count = i32(clouds.shadow_map.x);
    var next = -1;
    var previous = -1;
    var alpha = 0.0;
    for (var i = 0; i < 4; i++) {
        if i >= count {
            break;
        }
        var interval = clouds.shadow_intervals[i].xy;
        let center = (interval.x + interval.y) * 0.5;
        let edge = select(interval.y, interval.x, depth < center);
        let margin = edge * edge * 0.5;
        interval += margin * vec2<f32>(-0.5, 0.5);
        if i < count - 1 {
            if depth >= interval.x && depth < interval.y {
                previous = next;
                next = i;
                alpha = clamp(min(depth - interval.x, interval.y - depth) / margin, 0.0, 1.0);
            }
        } else if depth >= interval.x {
            // The last cascade does not fade out.
            previous = next;
            next = i;
            alpha = clamp((depth - interval.x) / margin, 0.0, 1.0);
        }
    }
    return select(previous, next, jitter <= alpha);
}

// The optical depth a cascade holds beyond `distance_offset` along the sun's ray (`readShadowOpticalDepth`).
fn read_shadow_optical_depth(uv: vec2<f32>, distance_to_top: f32, distance_offset: f32, cascade: i32) -> f32 {
    // r front depth (km), g mean extinction, b most optical depth, a its tail.
    let shadow = textureSampleLevel(shadow_maps, clamp_sampler, uv, cascade, 0.0);
    let distance_to_front = max(0.0, distance_to_top - distance_offset - shadow.r * 1000.0);
    return min(shadow.b + shadow.a, shadow.g * distance_to_front);
}

fn vogel_disk(index: i32, count: i32, phi: f32) -> vec2<f32> {
    let golden_angle = 2.39996322972865332;
    let r = sqrt(f32(index) + 0.5) / sqrt(f32(count));
    let theta = f32(index) * golden_angle + phi;
    return r * vec2<f32>(cos(theta), sin(theta));
}

fn shadow_optical_depth_pcf(world: vec3<f32>, distance_to_top: f32, distance_offset: f32, radius: f32, cascade: i32, pixel: vec2<f32>) -> f32 {
    let uv = shadow_uv(world, cascade);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return 0.0;
    }
    if radius < 0.1 {
        return read_shadow_optical_depth(uv, distance_to_top, distance_offset, cascade);
    }
    let count = i32(clouds.shadow_map.w);
    let texel = 1.0 / clouds.shadow_map.y;
    let phi = interleaved_gradient_noise(pixel) * PI2;
    var sum = 0.0;
    for (var i = 0; i < 16; i++) {
        if i >= count {
            break;
        }
        let offset = vogel_disk(i, count, phi);
        sum += read_shadow_optical_depth(uv + offset * radius * texel, distance_to_top, distance_offset, cascade);
    }
    return sum / f32(count);
}

// The optical depth toward the sun the shadow maps hold at a point, beyond `distance_offset` (`sampleShadowOpticalDepth`).
fn shadow_optical_depth(position: vec3<f32>, distance_offset: f32, radius: f32, jitter: f32, pixel: vec2<f32>) -> f32 {
    if clouds.experiments.y > 0.5 {
        return 0.0;
    }
    let distance_to_top = distance_to_shadow_top(position);
    if distance_to_top <= 0.0 {
        return 0.0;
    }
    let world = planet_to_world(position);
    let cascade = faded_cascade_index(world, jitter);
    if cascade < 0 {
        return 0.0;
    }
    return shadow_optical_depth_pcf(world, distance_to_top, distance_offset, radius, cascade, pixel);
}

// ---- Geometry -------------------------------------------------------------------------------------------------

struct Intersections {
    first: vec4<f32>,
    second: vec4<f32>,
}

// A ray's intersections with four spheres about the planet's centre (−1 where it misses one).
fn ray_spheres(origin: vec3<f32>, direction: vec3<f32>, radii: vec4<f32>) -> Intersections {
    let b = 2.0 * dot(direction, origin);
    let c = dot(origin, origin) - radii * radii;
    let discriminant = b * b - 4.0 * c;
    let miss = discriminant <= vec4<f32>(0.0);
    let q = sqrt(max(vec4<f32>(0.0), discriminant));
    return Intersections(
        select((-b - q) * 0.5, vec4<f32>(-1.0), miss),
        select((-b + q) * 0.5, vec4<f32>(-1.0), miss),
    );
}

fn ray_intersects_ground(origin: vec3<f32>, direction: vec3<f32>) -> bool {
    let r = length(origin);
    let mu = dot(origin, direction) / r;
    let bottom = clouds.sun.w;
    return mu < 0.0 && r * r * (mu * mu - 1.0) + bottom * bottom >= 0.0;
}

// ---- Phase and scattering -----------------------------------------------------------------------------------------

fn henyey_greenstein(g: vec2<f32>, cos_theta: f32) -> vec2<f32> {
    let g2 = g * g;
    return RECIPROCAL_PI4 * ((1.0 - g2) / max(vec2<f32>(1e-7), pow(1.0 + g2 - 2.0 * g * cos_theta, vec2<f32>(1.5))));
}

// The dual-lobe phase with its anisotropy attenuated by `attenuation` (Frostbite's octaves attenuate the phase angle
// instead).
fn phase_function(cos_theta: f32, attenuation: f32) -> f32 {
    let g = clouds.phase.xy;
    let weights = vec2<f32>(1.0 - clouds.phase.z, clouds.phase.z);
    return dot(henyey_greenstein(g * attenuation, cos_theta), weights);
}

// takram's multiple scattering (Wrenninge et al., "Oz: The Great and Volumetric", 2013): eight octaves, each a half of
// the one before in its contribution, in the optical depth toward the light it counts and in its phase's anisotropy.
// What it leaves is slabs' (`later_slab_octaves`) and a lightning flash's light; broken cloud takes `sun_scattering`.
const SLAB_OCTAVES: i32 = 8;

// takram's octaves after the first.
fn later_slab_octaves(optical_depth: f32, cos_theta: f32) -> f32 {
    var scale = 0.5;
    var scattering = 0.0;
    for (var i = 1; i < SLAB_OCTAVES; i++) {
        scattering += scale * exp(-optical_depth * scale) * phase_function(cos_theta, scale);
        scale *= 0.5;
    }
    return scattering;
}

// takram's eight octaves for a light at `cos_theta` from the view, through `optical_depth` toward it: the light a point
// of the medium scatters toward the eye per unit of the light's irradiance and of the scattering coefficient.
fn approximate_multiple_scattering(optical_depth: f32, cos_theta: f32) -> f32 {
    return exp(-optical_depth) * phase_function(cos_theta, 1.0) + later_slab_octaves(optical_depth, cos_theta);
}

// The octaves of broken cloud (cumulus, a storm's towers; 2026-10-04). Wrenninge named the octave sum "the contrast
// approximation" (2015): with takram's eight at a half each, the first octave, the only one with the true shadow and
// the true phase, is half the sum, and a quarter of it lies in octaves whose optical depth is cut 8 to 128 times, so
// every top was evenly white and a cloud against a low sun one dull mass. Shipped renderers use one to three (Frostbite
// and Red Dead Redemption 2 two, Unreal one, Dagor three at a contribution of 0.7, an attenuation of 0.3 and an
// eccentricity attenuation of 0.6: "at noon 0.5 to 0.6 and 0.45"). Here octave i carries OCTAVE_CONTRIBUTION^i of the
// light through OCTAVE_ATTENUATION^i of the optical depth by a phase whose anisotropy is cut to
// OCTAVE_PHASE_ATTENUATION^i: the later octaves light the first few optical depths (a rim glows against a low sun),
// and from about ten a cloud's core takes only the sky's light. The contribution holds a sunlit top's brightness where
// takram's was (with the edge term below, which takes two fifths of the later octaves out of the shell the eye sees);
// the attenuation is as steep as it is for the shadows' contrast (from 2.6 km at noon the tops' relief is 9.7 levels
// at 0.45, 10.1 at 0.6 and 10.3 at 0.75, takram's 9.6; `native/docs/clouds.md`).
const OCTAVE_CONTRIBUTION: f32 = 0.8;
const OCTAVE_ATTENUATION: f32 = 0.7;
const OCTAVE_PHASE_ATTENUATION: f32 = 0.6;

// A narrow forward lobe on the first octave (Guerrilla 2017: max(HG(g), silver_intensity × HG(0.99 − silver_spread))):
// a cloud's thin edges toward the sun, where the optical depth to the sun and to the eye are both small, light up as a
// droplet's forward peak lights them, which no lobe wide enough for midday holds. takram's forward lobe (g 0.7 at half
// weight) passes 0.28 of the light per steradian 20° from the sun and 0.55 at 10°; this one 0.40 and 1.23, the same from
// about 28° out. The first octave only: its full extinction keeps the lobe to the edges and the core dark.
const SILVER_INTENSITY: f32 = 0.7;
const SILVER_ANISOTROPY: f32 = 0.8;

// A cloud's edge scatters less of the sun toward the eye than its body does: the octaves after the first are light
// scattered more than once, which gathers with the depth of cloud about a sample (the darker rims of a billow against
// its lit face, which the octaves alone do not give). Keyed on how far the view ray has come into the cloud, the
// opacity gathered in front of the sample (Dagor's edge albedo: a ramp on the local density is one darkening everywhere,
// since the shell the eye sees is thin throughout; takram's powder term was such a ramp): EDGE_ALBEDO of the later
// octaves at the ray's entry, all of them from EDGE_DEPTH of opacity; no less of them looking toward the sun (cos⁸), so
// the silver lining stays. The sun's light only: darkening the sky's light lowers a cloud's brightness without adding
// contrast.
const EDGE_ALBEDO: f32 = 0.0;
const EDGE_DEPTH: f32 = 0.8;
// A layer too thin to gather light (a high sheet, rain: an optical depth under LAYER_BODY_DEPTH at its thickest; a
// cumulus is a hundred) scatters the sun once, by the forward lobe: in proportion to its thinness it has neither the
// back lobe, which stands for the light a body returns toward the sun, nor the narrow lobe, nor the later octaves.
// And of the forward lobe a veil keeps VEIL_FORWARD: ice crystals and rain drops, far larger than a cloud's droplets,
// send half of what they scatter into a diffraction peak too narrow to tell from the sun. (takram's powder term left
// the high sheet about a third of its light at every angle, and its density was chosen with that.)
const LAYER_BODY_DEPTH: f32 = 8.0;
const VEIL_FORWARD: f32 = 0.5;

// How much of a body each layer has, 0 a thin veil to 1 a cloud that gathers light.
fn layer_bodies() -> vec4<f32> {
    let thickness = clouds.density_scales * (clouds.max_layer_heights - clouds.min_layer_heights) * (clouds.medium.x + clouds.medium.y);
    return saturate4(thickness / LAYER_BODY_DEPTH);
}

// What a lit sample needs of its ray (the angle to the sun is one along a ray, so a march takes this once): the first
// octave's forward lobe toward the eye and what a body adds to it (the back lobe, and what the narrow lobe stands over
// the forward one); the later octaves' contributions times their phases (0 beyond the tier's octaves, `clouds.phase.w`,
// four at most); how much of the edge darkening the ray keeps (none toward the sun); and the angle's cosine.
struct RayLight {
    forward: f32,
    body: f32,
    later: vec3<f32>,
    edge: f32,
    cos_theta: f32,
}

fn ray_light(cos_theta: f32) -> RayLight {
    let lobes = henyey_greenstein(clouds.phase.xy, cos_theta) * vec2<f32>(1.0 - clouds.phase.z, clouds.phase.z);
    let silver = SILVER_INTENSITY * henyey_greenstein(vec2<f32>(SILVER_ANISOTROPY), cos_theta).x;
    let octaves = i32(clouds.phase.w);
    var later = array<f32, 3>(0.0, 0.0, 0.0);
    var contribution = OCTAVE_CONTRIBUTION;
    var anisotropy = OCTAVE_PHASE_ATTENUATION;
    for (var i = 1; i < 4; i++) {
        if i >= octaves {
            break;
        }
        later[i - 1] = contribution * phase_function(cos_theta, anisotropy);
        contribution *= OCTAVE_CONTRIBUTION;
        anisotropy *= OCTAVE_PHASE_ATTENUATION;
    }
    let toward = clamp(cos_theta, 0.0, 1.0);
    let toward2 = toward * toward;
    let toward4 = toward2 * toward2;
    return RayLight(
        lobes.x,
        lobes.y + max(silver - lobes.x, 0.0),
        vec3<f32>(later[0], later[1], later[2]),
        1.0 - toward4 * toward4,
        cos_theta,
    );
}

// The edge's ramp, 0 at a ray's entry into cloud to 1 from EDGE_DEPTH of opacity, over a sample's stretch of the view
// ray: `opacity` is what the ray has gathered in front of the stretch and by its end (0 to 1). The march counts a
// sample's light over its whole stretch, each part by the opacity it adds, so the ramp is its mean over that span of
// opacity: a march of long steps (the environment's bake, a coarse tier) then draws the edge a march of short ones does,
// where the ramp at the stretch's start would leave the first sample of a dense cloud, most of what the eye sees of it,
// without its later octaves.
fn edge_ramp(opacity: vec2<f32>) -> f32 {
    let span = opacity.y - opacity.x;
    if span < 1e-3 {
        return clamp(0.5 * (opacity.x + opacity.y) / EDGE_DEPTH, 0.0, 1.0);
    }
    // The ramp's integral from 0: o² / 2D up to D, then D / 2 + (o − D).
    let below = min(opacity, vec2<f32>(EDGE_DEPTH));
    let integral = below * below / (2.0 * EDGE_DEPTH) + max(opacity - EDGE_DEPTH, vec2<f32>(0.0));
    return clamp((integral.y - integral.x) / span, 0.0, 1.0);
}

// The sun's light a sample scatters toward the eye per unit of its irradiance there and of the scattering coefficient,
// through `optical_depth` toward the sun. The first octave: the forward lobe (a veil's share of it, VEIL_FORWARD), and by
// how much of a body the sample's layers have (`layer_bodies`) the back and narrow lobes. The later octaves, by that body too: of broken cloud three
// at most, less where the view ray has gathered little opacity by the sample (`opacity`, `edge_ramp`); of a slab
// (a deck, fog: `naval_clouds::layers::Layer::slab`) takram's seven, whole. Slabs keep takram's light until the light
// volume gives them their true optical depths: under a higher sheet (an optical depth of a few, which stops broken
// cloud's three octaves) takram's long tail is what reaches a deck's top, the deck's own light from above
// (`deck_skylight`) was derived against it, and the creases on a deck's top are takram's powder (`sample_radiance`).
fn sun_scattering(optical_depth: f32, opacity: vec2<f32>, media: MediaSample, ray: RayLight) -> f32 {
    let body = dot(layer_bodies(), media.weight);
    let slab = dot(clouds.layer_slabs, media.weight);
    let a = OCTAVE_ATTENUATION;
    let edge = mix(EDGE_ALBEDO, 1.0, edge_ramp(opacity));
    var later = mix(1.0, edge, ray.edge) * dot(ray.later, exp(-optical_depth * vec3<f32>(a, a * a, a * a * a)));
    if slab > 0.0 {
        later = mix(later, later_slab_octaves(optical_depth, ray.cos_theta), slab);
    }
    let first = ray.forward * mix(VEIL_FORWARD, 1.0, body) + body * ray.body;
    return first * exp(-optical_depth) + body * later;
}

// ---- Lightning ------------------------------------------------------------------------------------------------

// The flash's colour (the stroke's light on meshes, `naval_render::PointLight::lightning`).
const FLASH_TINT: vec3<f32> = vec3<f32>(0.8, 0.87, 1.0);
// Closer than this (m) to the channel's brightest point its falloff stops: the channel is hundreds of metres long.
const FLASH_CORE: f32 = 60.0;
// Samples of the cloud between a point and the flash.
const FLASH_STEPS: i32 = 3;
// The asymmetry of a cloud droplet's phase (water droplets of about 10 µm in visible light; Bohren 1987), which sets how
// far light diffuses through a cloud. takram's dual lobe is fitted to the clouds' look, not to their transmission.
const DROPLET_ASYMMETRY: f32 = 0.85;

// The cloud's extinction (per metre) at a planet-frame point from its weather alone: the layers' coverage and density
// profiles before the shape and detail carve them, the medium a flash's light crosses on its way, smooth at the scale
// of a stroke's reach.
fn coarse_extinction(position: vec3<f32>, mip_level: f32) -> f32 {
    let weather = sample_weather(weather_uv(position), length(position) - clouds.sun.w, mip_level, false);
    let density = saturate4(weather.density * clouds.density_scales * layer_density(weather.height_fraction));
    return dot(density, vec4<f32>(1.0)) * (clouds.medium.x + clouds.medium.y);
}

// A lightning flash's light scattered toward the eye (along `ray_direction`) at a planet-frame point (m), before the
// medium's scattering coefficient, as the sun's term in `march_clouds`: the channel a point of the sky's
// `lightning_intensity` (an irradiance of `intensity × (1 km / r)²` in the sea's units) at `lightning_position`, its light
// through the cloud between (the weather's extinction over FLASH_STEPS jittered samples) by the same octaves of multiple
// scattering and phase as the sun's; deep in the cloud, where the octaves' fall-off leaves nothing, by the light a cloud
// that absorbs none diffuses through itself (the two-stream slab transmission 1 / (1 + ¾ (1 − g) τ), g a cloud droplet's
// asymmetry, DROPLET_ASYMMETRY), spread evenly: a stroke inside a deck lights it from within and its base from above.
fn flash_radiance(position: vec3<f32>, ray_direction: vec3<f32>, mip_level: f32, jitter: f32) -> vec3<f32> {
    let to_flash = world_to_planet(sky.lightning_position) - position;
    let reach = length(to_flash);
    let direction = to_flash / max(reach, 1e-3);
    let step = reach / f32(FLASH_STEPS);
    var optical_depth = 0.0;
    for (var i = 0; i < FLASH_STEPS; i++) {
        optical_depth += coarse_extinction(position + direction * ((f32(i) + jitter) * step), mip_level + 1.0) * step;
    }
    let distance = max(reach, FLASH_CORE);
    let irradiance = sky.lightning_intensity * 1e6 / (distance * distance);
    let diffused = RECIPROCAL_PI4 / (1.0 + 0.75 * (1.0 - DROPLET_ASYMMETRY) * optical_depth);
    let scattered = approximate_multiple_scattering(optical_depth, dot(direction, ray_direction));
    return FLASH_TINT * (irradiance * max(scattered, diffused));
}

// The optical depth along a short march from a point (`marchOpticalDepth`); `ray_distance` is the last sample's.
struct OpticalDepth {
    value: f32,
    ray_distance: f32,
}

fn march_optical_depth(origin: vec3<f32>, direction: vec3<f32>, max_iterations: i32, mip_level: f32, jitter: f32) -> OpticalDepth {
    let iterations = i32(max(0.0, mix(f32(max_iterations + 1), 1.0, mip_level) - jitter));
    if iterations == 0 {
        // takram's fudge for the mean optical depth.
        return OpticalDepth(0.5, 0.0);
    }
    var step_size = clouds.secondary.z / f32(iterations);
    var next_distance = step_size * jitter;
    var optical_depth = 0.0;
    var ray_distance = 0.0;
    for (var i = 0; i < iterations; i++) {
        ray_distance = next_distance;
        let position = ray_distance * direction + origin;
        let uv = weather_uv(position);
        let height = length(position) - clouds.sun.w;
        let weather = sample_weather(uv, height, mip_level, false);
        let media = sample_media(weather, position, uv, mip_level, jitter);
        optical_depth += media.extinction * step_size;
        next_distance += step_size;
        step_size *= clouds.secondary.w;
    }
    return OpticalDepth(optical_depth, ray_distance);
}

// The sun's and sky's light at the camera and through the layers, sampled once per ray where the light is not
// evaluated per sample (takram's per-vertex `vGroundIrradiance` and `vCloudsIrradiance`).
struct FrameLight {
    ground: SunSky,
    lowest: SunSky,
    highest: SunSky,
}

fn frame_light(camera: vec3<f32>) -> FrameLight {
    let up = normalize(camera);
    // Fog and rain start under the sea (`naval_clouds::layers::Layer`); the atmosphere's light ends at its bottom.
    let radii = clouds.sun.w + max(clouds.heights.xy, vec2<f32>(0.0));
    return FrameLight(
        key_scalar_irradiance(camera * METER_TO_KM),
        key_scalar_irradiance(up * radii.x * METER_TO_KM),
        key_scalar_irradiance(up * radii.y * METER_TO_KM),
    );
}

fn clouds_sun_sky_irradiance(position: vec3<f32>, height: f32, light: FrameLight) -> SunSky {
    if clouds.switches.w > 0.5 {
        return key_scalar_irradiance(position * METER_TO_KM);
    }
    let alpha = remap_clamped(height, clouds.heights.x, clouds.heights.y);
    return SunSky(mix(light.lowest.sun, light.highest.sun, alpha), mix(light.lowest.skylight, light.highest.skylight, alpha));
}

// The diffusion of light through a slab of cloud (two-stream, conservative scattering with the droplets' asymmetry
// g = 0.85): the share of the light falling on its top that leaves its bottom diffusely, 1 / (1 + 3/4 (1 − g) τ).
const SLAB_DIFFUSION: f32 = 0.1125;

// The light a point under or inside a closed deck takes from above, in place of takram's sky light (which counts the
// whole clear sky over every point and leaves an overcast's base one evenly lit sheet): the sky's and the sun's light
// on the deck's top diffused down through the cloud over the point, less the sun's beam that crosses it unscattered
// (the octaves' share). Thick patches of deck darken the light under them, thin ones and the gaps between pass more:
// the deck's structure seen from below. Weighed in by how closed the deck above is (`ColumnAbove.closed`), so broken
// cloud keeps takram's light. Irradiance, as `irradiance.skylight`.
fn deck_skylight(skylight: vec3<f32>, sun: vec3<f32>, column: ColumnAbove, surface_normal: vec3<f32>) -> vec3<f32> {
    let mu = max(dot(clouds.sun.xyz, surface_normal), 0.0);
    let diffused = 1.0 / (1.0 + SLAB_DIFFUSION * column.optical_depth);
    let beam = exp(-column.optical_depth / max(mu, 0.05));
    return max((skylight + sun * mu) * diffused - sun * mu * beam, vec3<f32>(0.0));
}

// Light bounced off the sea below a point (takram's ground bounce: an albedo of 0.3 under the sky's light and the sun's
// in the gaps between the clouds), through the cloud under it. takram marches toward the ground for that cloud (three
// steps, one on its Medium preset: 100 m of it), which a tier may still ask (`clouds.secondary.y`); none does, and the
// cloud under a sample is its own layers' weather at the sample, down to their bases, GROUND_FILL of it: nothing at a
// cloud's base and its sides, the whole depth inside it. (A tenth: the weather thins toward a layer's base and the noise
// carves its thin parts away, so little of the depth under a sample near a cloud's underside holds cloud. With it the
// clouds from the sea are within a level of the marched step's in fair weather and under a deck, 1.3 under the storm.)
// Without the sun's deep octaves the bounce is about half a cloud base's light, so every tier takes it (takram's High
// preset here took none from 2026-10-02, and drew its bases darker than Medium's), and every tier by the frame's light
// at the camera (`FrameLight.ground`, takram's without accurate light): the sea's own under each sample, which
// takram's accurate light takes, costs High an overcast 0.4 ms for a light that is a guess already.
const GROUND_FILL: f32 = 0.1;

fn radiance_from_ground(position: vec3<f32>, surface_normal: vec3<f32>, weather: WeatherSample, mip_level: f32, jitter: f32, light: FrameLight) -> vec3<f32> {
    var optical_depth = 0.0;
    let iterations = i32(clouds.secondary.y);
    if iterations > 0 {
        optical_depth = march_optical_depth(position, -surface_normal, iterations, mip_level, jitter).value;
    } else {
        let extinction = saturate4(weather.density * clouds.density_scales * layer_density(weather.height_fraction)) * (clouds.medium.x + clouds.medium.y);
        let below = weather.height_fraction * (clouds.max_layer_heights - clouds.min_layer_heights);
        optical_depth = GROUND_FILL * dot(extinction, below);
    }
    let ground_albedo = 0.3;
    let ground_irradiance = light.ground.skylight + (1.0 - clouds.medium.z) * light.ground.sun;
    let bounced = ground_albedo * RECIPROCAL_PI * ground_irradiance;
    return bounced * exp(-optical_depth);
}

// The share of the sky's light the cloud about a sample lets reach it (Guerrilla 2022: pow(1 − profile, 0.5), the
// profile a field rising from 0 at a cloud's outside to 1 in its core). The profile here is the sample's layer's weather
// before the shape and detail noise carve it (`WeatherSample.density`): 0 at the envelope's edge, top and base, rising
// into the cell. The shape noise puts the surface the eye sees where that value equals what it carves (1 − shape, times
// the layer's shape amount), so hollows, carved deep into the envelope, take less of the sky than the billows standing
// out of it. A layer too thin to hide the sky (a high sheet, rain: `layer_bodies`) hides in proportion. Never under
// AMBIENT_FLOOR: the sky's light diffuses into a core as the sun's does.
const AMBIENT_FLOOR: f32 = 0.4;

fn sky_occlusion(weather: WeatherSample, media: MediaSample) -> f32 {
    let profile = clamp(dot(weather.density * layer_bodies(), media.weight), 0.0, 1.0);
    return mix(AMBIENT_FLOOR, 1.0, sqrt(1.0 - profile));
}

// The sky's light on a sample (irradiance, as `SunSky.skylight`): a crude gradient of the sky over the layer's height
// (takram's, better than none in the shadows) less what the cloud about the sample hides of it; under a closed deck,
// the light diffused down through it (`deck_skylight`, weighed in by `ColumnAbove.closed`).
fn sky_light(irradiance: SunSky, weather: WeatherSample, media: MediaSample, column: ColumnAbove, surface_normal: vec3<f32>) -> vec3<f32> {
    let sky_gradient = dot(weather.height_fraction * 0.5 + 0.5, media.weight);
    var skylight = irradiance.skylight * (sky_gradient * sky_occlusion(weather, media));
    if column.closed > 0.0 {
        skylight = mix(skylight, deck_skylight(irradiance.skylight, irradiance.sun, column, surface_normal), column.closed);
    }
    return skylight;
}

// The optical depth from a sample toward the sun: a short march for the detail the shadow maps lack, then the shadow
// maps' beyond what was marched (filtered only with the sun near the horizon).
fn sun_optical_depth(position: vec3<f32>, height: f32, surface_normal: vec3<f32>, mip_level: f32, jitter: f32, pixel: vec2<f32>) -> f32 {
    // The zero-step experiment keeps the native mean-depth approximation but guarantees no local texture march.
    var toward_sun = OpticalDepth(0.5, 0.0);
    if clouds.experiments.w < 0.5 {
        toward_sun = march_optical_depth(position, clouds.sun.xyz, i32(clouds.secondary.x), mip_level, jitter);
    }
    var optical_depth = toward_sun.value;
    if height < clouds.heights.z {
        optical_depth += shadow_optical_depth(
            position,
            toward_sun.ray_distance,
            clouds.shadow_map.z * remap_clamped(dot(clouds.sun.xyz, surface_normal), 0.1, 0.0),
            jitter,
            pixel,
        );
    }
    return optical_depth;
}

// Under a closed deck this deep in optical depth the sun's beam and its octaves of multiple scattering (a slab's, about
// 1 / (e τ) of it; broken cloud's three pass less) are under a fiftieth of the light the deck diffuses down
// (`deck_skylight`): the sun is not marched.
const SPENT_DEPTH: f32 = 60.0;

// The light a sample of the medium scatters toward the eye, per metre of the view ray (the source term the march
// integrates), for any march of the clouds (the far march, a near pass): the key light through the cloud toward it by
// the octaves (`sun_scattering`), the sea's bounce, the sky's light (`sky_light`) and a lightning flash. `position` is in
// the planet frame, `height` above the sea, `uv` its weather coordinates (`weather_uv`), `opacity` what the view ray
// has gathered in front of the sample's stretch and by its end (0 to 1: 1 − the transmittance before the stretch, and
// after it), `ray` and `light` the ray's (`ray_light`, `frame_light`), `pixel` the drawing buffer's pixel (the shadow
// filter's rotation).
fn sample_radiance(
    position: vec3<f32>,
    height: f32,
    uv: vec2<f32>,
    weather: WeatherSample,
    media: MediaSample,
    mip_level: f32,
    jitter: f32,
    ray_direction: vec3<f32>,
    opacity: vec2<f32>,
    ray: RayLight,
    light: FrameLight,
    pixel: vec2<f32>,
) -> vec3<f32> {
    let irradiance = clouds_sun_sky_irradiance(position, height, light);
    let surface_normal = normalize(position);
    var column = ColumnAbove(0.0, 0.0);
    if any_deck() {
        column = column_above(uv, height, mip_level);
    }

    // The sun through the cloud toward it. Deep under a closed deck the beam's octaves are spent and are not marched.
    var radiance = vec3<f32>(0.0);
    if column.closed < 1.0 || column.optical_depth < SPENT_DEPTH {
        let optical_depth = sun_optical_depth(position, height, surface_normal, mip_level, jitter, pixel);
        radiance = irradiance.sun * sun_scattering(optical_depth, opacity, media, ray);
    }

    // The fudge for the light from the ground.
    if clouds.state.x > 0.5 && height < clouds.heights.z && mip_level < 0.5 {
        let ground = radiance_from_ground(position, surface_normal, weather, mip_level, jitter, light);
        radiance += ground * RECIPROCAL_PI4 * clouds.scattering.y;
    }

    radiance += sky_light(irradiance, weather, media, column, surface_normal) * RECIPROCAL_PI4 * clouds.scattering.x;

    // A lightning flash lights the cloud about its channel.
    if sky.lightning_intensity > 0.0 {
        radiance += flash_radiance(position, ray_direction, mip_level, jitter);
    }
    radiance *= media.scattering;

    // takram's powder, on slabs only (`sun_scattering`): the thin parts of a deck or of fog scatter less of every light.
    let slab = dot(clouds.layer_slabs, media.weight);
    if clouds.state.y > 0.5 && slab > 0.0 {
        radiance *= 1.0 - slab * clouds.scattering.z * exp(-media.extinction * clouds.scattering.w);
    }
    return radiance;
}

// ---- The primary march ------------------------------------------------------------------------------------------

// The fewest samples a steep ray takes through the shadow-casting layers' depth.
const SHELL_SAMPLES: f32 = 16.0;
// Within FINE_REACH (m) of the camera, a step through the medium takes at most FINE_DEPTH of optical depth, and no step
// is shorter than FINE_STEP (m).
const FINE_REACH: f32 = 600.0;
const FINE_DEPTH: f32 = 0.35;
const FINE_STEP: f32 = 8.0;
// A cloud (not a slab: those keep FINE_DEPTH) within NEAR_STEP_REACH.x metres of the camera is walked in steps of
// NEAR_STEP (m), the steps growing back to takram's by NEAR_STEP_REACH.y. A camera travelling past a cumulus a few
// hundred metres off showed the cloud as grain: its 50 m steps put each sample's edge and depth somewhere else, and the
// reconstruction, which a moving view leaves a few samples a pixel, could not settle them (2026-10-05, "A camera
// travelling near the clouds" in `clouds.md`). Shorter steps than NEAR_STEP measured no better; Medium pays 0.15 ms
// among cumulus at 1120 × 630, nothing where no cloud is that near.
const NEAR_STEP: f32 = 20.0;
const NEAR_STEP_REACH: vec2<f32> = vec2<f32>(800.0, 1500.0);
// Fog and rain lie in a band from under the sea up (the fourth layer, starting under the sea:
// `naval_clouds::layers::Layer`), which a level ray crosses for tens of kilometres: its empty air is crossed in strides of
// SURFACE_STRIDE of the distance (rain shafts and fog banks hold no detail a step there could miss), and beyond
// SURFACE_REACH (m) the rest of the band is skipped.
const SURFACE_STRIDE: f32 = 0.06;
const SURFACE_REACH: f32 = 12000.0;

// Diagnostic colours are accumulated with the actual primary-ray extinction, so an uncovered *sample* is visible
// even when the representative front depth would fall in another cascade. These modes do not change density.
fn cloud_debug_color(position: vec3<f32>, height: f32, mip_level: f32, jitter: f32, pixel: vec2<f32>) -> vec3<f32> {
    let mode = i32(clouds.experiments.x);
    if mode == 1 { return vec3<f32>(1.0); }
    if mode == 2 {
        let tau = sun_optical_depth(position, height, normalize(position), mip_level, jitter, pixel);
        return vec3<f32>(exp(-tau));
    }
    let world = planet_to_world(position);
    let cascade = faded_cascade_index(world, jitter);
    if cascade < 0 { return vec3<f32>(1.0, 0.0, 1.0); }
    if mode == 3 {
        let palette = array<vec3<f32>, 4>(vec3<f32>(0.1, 1.0, 0.15), vec3<f32>(0.15, 0.4, 1.0),
            vec3<f32>(1.0, 0.8, 0.1), vec3<f32>(0.8, 0.1, 1.0));
        return palette[cascade];
    }
    let uv = shadow_uv(world, cascade);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return vec3<f32>(1.0, 0.03, 0.0);
    }
    let radius = clouds.shadow_map.z * remap_clamped(dot(clouds.sun.xyz, normalize(position)), 0.1, 0.0);
    // Orange warns that filtering can reach the edge even though the centre is inside.
    let border = (radius + 0.5) / clouds.shadow_map.y;
    if any(uv < vec2<f32>(border)) || any(uv > vec2<f32>(1.0 - border)) {
        return vec3<f32>(1.0, 0.4, 0.0);
    }
    return vec3<f32>(0.03, 1.0, 0.1);
}

struct CloudMarch {
    // Premultiplied radiance, and opacity (`remapClamped(transmittance, 1, minTransmittance)`).
    color: vec4<f32>,
    // The transmittance-weighted mean distance of what was marched (m from the ray's start), or −1 for none.
    front_depth: f32,
}

fn march_clouds(
    ray_origin: vec3<f32>,
    ray_direction: vec3<f32>,
    ray_near_far: vec2<f32>,
    cos_theta: f32,
    jitter: f32,
    ray_start_texels_per_pixel: f32,
    light: FrameLight,
    pixel: vec2<f32>,
) -> CloudMarch {
    var radiance_integral = vec3<f32>(0.0);
    var transmittance_integral = 1.0;
    var weighted_distance_sum = 0.0;
    var transmittance_sum = 0.0;
    let ray = ray_light(cos_theta);

    let max_ray_distance = ray_near_far.y - ray_near_far.x;
    let perspective = clouds.march_limits.x;
    // Only rays whose cloud entry is far away take this approximation. Close cloud flights keep native steps.
    let far_scale = mix(1.0, clouds.experiments.z, smoothstep(1500.0, 5000.0, ray_near_far.x));
    let min_step = clouds.march.y * far_scale;
    let max_step = clouds.march.z * far_scale;
    var step_size = (clouds.march.y + (perspective - 1.0) * ray_near_far.x) * far_scale;
    // takram's step grows with the distance to the layers, which from far above (the chart, 40 km up) leaves a steep
    // ray four samples through the low clouds and every pixel a different four. A steep ray takes at least
    // SHELL_SAMPLES through the shadow-casting layers' depth; level rays keep takram's step.
    let vertical = max(abs(dot(normalize(ray_origin), ray_direction)), 1e-3);
    let shell = max(clouds.heights.z - clouds.heights.w, min_step);
    step_size = min(step_size, max(min_step, shell / (SHELL_SAMPLES * vertical)));
    // takram: spatial aliasing remains unless the jitter is doubled.
    var ray_distance = step_size * jitter * 2.0;
    let max_iterations = i32(clouds.march.x);
    // The top of the fog and rain band, or below everything without one.
    let surface_top = select(-1e9, clouds.max_layer_heights.w, clouds.min_layer_heights.w < 0.0);

    for (var i = 0; i < max_iterations; i++) {
        if ray_distance > max_ray_distance {
            break;
        }
        let position = ray_distance * ray_direction + ray_origin;
        let height = length(position) - clouds.sun.w;
        let mip_level = log2(max(1.0, ray_start_texels_per_pixel + ray_distance * 1e-5));
        let in_band = height < surface_top;

        if in_band && ray_distance > SURFACE_REACH {
            // Out of reach in the band: on to where the ray leaves it, upward (a ray going down meets the sea first).
            let exit = ray_sphere_second(ray_origin, ray_direction, clouds.sun.w + surface_top);
            step_size *= perspective;
            ray_distance = max(exit, ray_distance + step_size);
            continue;
        }

        if inside_layer_intervals(height) {
            step_size *= perspective;
            ray_distance += mix(step_size, max_step, min(1.0, mip_level));
            continue;
        }

        // The rough weather.
        let uv = weather_uv(position);
        let weather = sample_weather(uv, height, mip_level, false);
        if !any(weather.density > vec4<f32>(clouds.march_limits.y)) {
            // Longer steps through empty air.
            step_size *= perspective;
            let stride = select(0.0, SURFACE_STRIDE * ray_distance, in_band);
            ray_distance += max(mix(step_size, max_step, min(1.0, mip_level)), stride);
            continue;
        }

        // The detailed medium, its detail no finer than this step and this pixel show.
        let camera_distance = ray_near_far.x + ray_distance;
        let media = sample_media_at(weather, position, uv, mip_level, jitter, detail_lod(step_size, camera_distance));
        // Near the camera an optically thick step through a slab (fog the camera stands in, a deck it flies through) is
        // cut to FINE_DEPTH of optical depth, and the ray goes on by it: takram's 50 m steps there leave every pixel a
        // different few samples of a dense medium, noise the reconstruction cannot settle. The ray ends after a few such
        // steps (it is opaque), so the cost stays bounded. takram's own layers keep its steps.
        // And the other way round: a thin part of a slab (a deck's ragged base) is stepped through in up to two steps'
        // length, rain in the fog and rain band in up to four, FINE_DEPTH of optical depth each, as the light in them
        // changes no faster.
        var segment = step_size;
        var fine = false;
        let slab = in_band || dot(media.weight, clouds.layer_slabs) > 0.5;
        let near_step = mix(min(step_size, NEAR_STEP), step_size, smoothstep(NEAR_STEP_REACH.x, NEAR_STEP_REACH.y, camera_distance));
        if (slab || near_step < step_size) && media.extinction > clouds.march_limits.z {
            let even = FINE_DEPTH / media.extinction;
            let near = ray_distance < FINE_REACH;
            // A cloud near the camera takes no step longer than the near step.
            var most = select(1.0, select(2.0, 4.0, in_band), slab) * step_size;
            if !slab {
                most = min(most, near_step);
            }
            let least = min(min(select(step_size, FINE_STEP, slab && near), step_size), most);
            segment = clamp(even, least, most);
            fine = segment != step_size;
        }
        if media.extinction > clouds.march_limits.z {
            // The light the sample scatters toward the eye; its edge term takes the opacity the ray has gathered in
            // front of the sample's stretch and by its end.
            let transmittance = exp(-media.extinction * segment);
            var radiance: vec3<f32>;
            if clouds.experiments.x >= 1.0 && clouds.experiments.x <= 4.0 {
                radiance = cloud_debug_color(position, height, mip_level, jitter, pixel) * media.extinction;
            } else {
                radiance = sample_radiance(
                position,
                height,
                uv,
                weather,
                media,
                mip_level,
                jitter,
                ray_direction,
                1.0 - transmittance_integral * vec2<f32>(1.0, transmittance),
                ray,
                light,
                pixel,
                );
            }

            // Energy-conserving analytical integration of the scattered light (Frostbite 2016, 5.6.3).
            let clamped_extinction = max(media.extinction, 1e-7);
            let scattering_integral = (radiance - radiance * transmittance) / clamped_extinction;
            radiance_integral += transmittance_integral * scattering_integral;
            transmittance_integral *= transmittance;

            // The aerial perspective's depth (Frostbite 2016, 5.9.1).
            weighted_distance_sum += ray_distance * transmittance_integral;
            transmittance_sum += transmittance_integral;
        }

        if transmittance_integral <= clouds.march_limits.w {
            break;
        }

        // A shorter step inside the clouds.
        step_size *= perspective;
        ray_distance += select(step_size, segment, fine);
    }

    var result: CloudMarch;
    result.front_depth = select(-1.0, weighted_distance_sum / transmittance_sum, transmittance_sum > 0.0);
    result.color = vec4<f32>(radiance_integral, remap_clamped(transmittance_integral, 1.0, clouds.march_limits.w));
    return result;
}

// ---- Light shafts and haze ----------------------------------------------------------------------------------------

// The length (m) of the view ray the clouds' shadow covers (`marchShadowLength`).
fn march_shadow_length(ray_origin: vec3<f32>, ray_direction: vec3<f32>, ray_near_far: vec2<f32>, jitter: f32, pixel: vec2<f32>) -> f32 {
    var shadow_length = 0.0;
    let max_ray_distance = ray_near_far.y - ray_near_far.x;
    var step_size = clouds.shafts.y;
    var ray_distance = step_size * jitter;
    let attenuation = 1.0;
    let max_iterations = i32(clouds.shafts.x);
    for (var i = 0; i < max_iterations; i++) {
        if ray_distance > max_ray_distance {
            break;
        }
        let position = ray_distance * ray_direction + ray_origin;
        let optical_depth = shadow_optical_depth(position, 0.0, 0.0, jitter, pixel);
        shadow_length += (1.0 - exp(-optical_depth)) * step_size * attenuation;
        step_size *= clouds.march_limits.x;
        ray_distance += step_size;
    }
    return shadow_length;
}

// Sparse fog under the clouds (`approximateHaze`): in-scattered radiance (rgb) and its opacity (a).
fn approximate_haze(ray_origin: vec3<f32>, ray_direction: vec3<f32>, max_ray_distance: f32, cos_theta: f32, shadow_length: f32, light: FrameLight) -> vec4<f32> {
    let camera_height = clouds.camera.w;
    let modulation = remap_clamped(clouds.medium.z, 0.2, 0.4);
    if camera_height * modulation < 0.0 {
        return vec4<f32>(0.0);
    }
    let density = modulation * clouds.haze.x * exp(-camera_height * clouds.haze.y);
    if density < 1e-7 {
        return vec4<f32>(0.0);
    }
    // Blend two normals by their difference in angle: near the ground the origin's, in the sky the horizon's.
    let bottom = clouds.sun.w;
    let normal_at_origin = normalize(ray_origin);
    let normal_at_horizon = (ray_origin - dot(ray_origin, ray_direction) * ray_direction) / bottom;
    let alpha = remap_clamped(dot(normal_at_origin, normal_at_horizon), 0.9, 1.0);
    let normal = mix(normal_at_origin, normal_at_horizon, alpha);

    // Analytical optical depth through density falling exponentially with height (Quílez, "fog").
    let angle = max(dot(normal, ray_direction), 1e-5);
    let exponent = angle * clouds.haze.y;
    let linear_term = density / clouds.haze.y / angle;

    // The optical depths with and without the shadowed length.
    let exp_term = 1.0 - exp(-max_ray_distance * exponent);
    let shadow_exp_term = 1.0 - exp(-min(max_ray_distance, shadow_length) * exponent);
    let optical_depth = exp_term * linear_term;
    let shadow_optical_depth = max((exp_term - shadow_exp_term) * linear_term, 0.0);
    let transmittance = clamp(1.0 - exp(-optical_depth), 0.0, 1.0);
    let shadow_transmittance = clamp(1.0 - exp(-shadow_optical_depth), 0.0, 1.0);

    var inscatter = light.ground.sun * phase_function(cos_theta, 1.0) * shadow_transmittance;
    inscatter += light.ground.skylight * RECIPROCAL_PI4 * clouds.scattering.x * transmittance;
    inscatter *= clouds.haze.z / (clouds.haze.w + clouds.haze.z);
    return vec4<f32>(inscatter, transmittance);
}

// The atmosphere between the camera and the clouds' front over the clouds' colour. The clouds keep takram's radiance,
// on the same scale as the sky's: the eye adaptation, not a gain on the clouds alone, suits the exposure to them.
fn apply_aerial_perspective(camera: vec3<f32>, front: vec3<f32>, shadow_length: f32, color: vec4<f32>) -> vec4<f32> {
    let air = sky_radiance_to_point(camera * METER_TO_KM, front * METER_TO_KM, shadow_length * METER_TO_KM);
    return vec4<f32>(color.rgb * air.transmittance + air.radiance * color.a, color.a);
}

// ---- Ray ranges ---------------------------------------------------------------------------------------------------

struct RayRanges {
    ground: bool,
    first: vec4<f32>,
    second: vec4<f32>,
}

fn ray_ranges(camera: vec3<f32>, direction: vec3<f32>) -> RayRanges {
    let spheres = ray_spheres(camera, direction, clouds.sun.w + vec4<f32>(0.0, clouds.heights.x, clouds.heights.y, clouds.heights.z));
    return RayRanges(ray_intersects_ground(camera, direction), spheres.first, spheres.second);
}

// Where a ray that meets the sea leaves the layers going down: the lowest layer's bottom, or the sea when that lies
// under it (fog and rain start under the sea, `naval_clouds::layers::Layer`, and a grazing ray may miss its sphere).
fn layers_floor(ranges: RayRanges) -> f32 {
    let to_sea = ranges.first.x;
    return min(select(ranges.first.y, to_sea, ranges.first.y < 0.0), to_sea);
}

// The stretch of the view ray through the cloud layers (`getRayNearFar`); negative where the ray meets the sea first.
fn clouds_near_far(ranges: RayRanges) -> vec2<f32> {
    let camera_height = clouds.camera.w;
    var near_far: vec2<f32>;
    if camera_height < clouds.heights.x {
        // Below the clouds.
        if ranges.ground {
            near_far = vec2<f32>(-1.0);
        } else {
            near_far = vec2<f32>(ranges.second.y, min(ranges.second.z, clouds.march.w));
        }
    } else if camera_height < clouds.heights.y {
        // Inside the layers.
        near_far = vec2<f32>(clouds.planes.x, select(ranges.second.z, layers_floor(ranges), ranges.ground));
    } else {
        // Above the clouds.
        near_far = vec2<f32>(ranges.first.z, select(ranges.second.z, layers_floor(ranges), ranges.ground));
    }
    return near_far;
}

// The stretch of the view ray under the shadow-casting layers' top (`getShadowRayNearFar`).
fn shadow_near_far(ranges: RayRanges) -> vec2<f32> {
    let camera_height = clouds.camera.w;
    var near_far: vec2<f32>;
    if camera_height < clouds.heights.z {
        near_far = vec2<f32>(clouds.planes.x, select(ranges.second.w, ranges.first.x, ranges.ground));
    } else {
        near_far = vec2<f32>(ranges.first.w, select(ranges.second.w, ranges.first.x, ranges.ground));
    }
    near_far.y = min(near_far.y, clouds.shafts.z);
    return near_far;
}

// The stretch of the view ray in the haze (`getHazeRayNearFar`).
fn haze_near_far(ranges: RayRanges) -> vec2<f32> {
    return vec2<f32>(clouds.planes.x, select(ranges.second.z, ranges.first.x, ranges.ground));
}
