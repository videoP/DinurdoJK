// The bodies beyond the air: the limb-darkened sun and its corona, the moon lit by the true sun with a
// procedural near side, the stars from the catalogue's grids, and the Milky Way band. Radiance above the
// atmosphere; the dome multiplies it by the transmittance to space.
//
// Ports the web game's `sky/celestial/discs.ts` (`sunDisc`, `moonDisc`, `moonAlbedo`, `moonCover`, `moonGlow`),
// `starField.ts` (`StarField.radiance`), `milkyWay.ts` (`MilkyWay.sample`) and `CelestialBodies.ts`
// (`radiance`, `diffuseRadiance`).
#define_import_path naval_sky::celestial

#import naval_sky::common::{sky, star_grid, star_ramp, milky_way_map, milky_way_sampler, linear_clamp, PI}
#import naval_sky::noise::{mx_noise_float2, mx_noise_float3}

// Angular radius (radians) of the sun and the moon: 1 − cos θ = 7.5e-5.
const DISC_RADIUS: f32 = 0.01224744871391589;
const SUN_DISC: f32 = 1.35;
const SUN_LIMB: vec3<f32> = vec3<f32>(0.5, 0.6, 0.72);
const CORONA: f32 = 0.045;
const CORONA_WIDTH: f32 = 0.003490658503988659;
const EDGE_PX: f32 = 0.8;
const MOON_HALO_WIDTH: f32 = 0.0020943951023931952;

// Stars (`starField.ts`, `stars.ts`).
const STAR_CELLS: f32 = 80.0;
const STAR_MARGIN: f32 = 0.003316125578789226;
// A cell's layers (`stars::STAR_LAYERS`), the rows between them (four grids of 81), and the flag of a further one.
const STAR_LAYERS: i32 = 4;
const STAR_LAYER_ROWS: i32 = 324;
const STAR_MORE: f32 = 10.0;
const STAR_MORE_FROM: f32 = 5.0;
const STAR_PEAK: f32 = 1.0;
// 0.4 · COMPRESSION · log2(10), COMPRESSION = 0.55.
const STAR_FLUX_EXPONENT: f32 = 0.7308241808752198;
const SPREAD_PX: f32 = 0.8;
const BRIGHT_SPREAD: f32 = 0.3;
const TWINKLE_ZENITH: f32 = 0.1;
const TWINKLE_HORIZON: f32 = 0.6;
const COLOR_FADE: vec2<f32> = vec2<f32>(1.5, 5.5);
const COLOR_INDEX_LOW: f32 = -0.3;
const COLOR_INDEX_HIGH: f32 = 2.0;
const WARP_A: f32 = 0.31157;
const WARP_B: f32 = 0.08442;

// The Milky Way texture holds sqrt(radiance / ENCODING).
const MILKY_WAY_ENCODING: f32 = 2.0;
// The night sky's own light with no moon up: airglow and the stars too faint to draw, as a share of the Milky Way's
// brightest (`sky.milky_way`, which fades it through twilight and under a moon). It is what a ship and the sea are
// seen by on a moonless night; the real figure is nearer a tenth of this, raised so such a night can be played.
const NIGHT_GLOW: vec3<f32> = vec3<f32>(0.75, 1.15, 1.9);

// Share of the view along `direction` the moon's disc covers.
fn moon_cover(direction: vec3<f32>, moon: vec3<f32>, pixel: f32) -> f32 {
    let offset = direction - moon;
    return smoothstep(-pixel * EDGE_PX, pixel * EDGE_PX, -(sqrt(dot(offset, offset)) - DISC_RADIUS));
}

// Limb-darkened sun and its corona along `direction`, behind the moon.
fn sun_disc(direction: vec3<f32>, pixel: f32) -> vec3<f32> {
    let reach = DISC_RADIUS + CORONA_WIDTH * 9.0;
    let offset = direction - sky.sun_direction;
    let angle2 = dot(offset, offset);
    if angle2 >= reach * reach {
        return vec3<f32>(0.0);
    }
    let angle = sqrt(angle2);
    let r = min(angle / DISC_RADIUS, 1.0);
    let mu = sqrt(1.0 - r * r);
    let limb = vec3<f32>(1.0) - SUN_LIMB * (1.0 - mu);
    let edge = smoothstep(-pixel * EDGE_PX, pixel * EDGE_PX, -(angle - DISC_RADIUS));
    let corona = exp(max(angle - DISC_RADIUS, 0.0) / -CORONA_WIDTH) * CORONA;
    return sky.sun_irradiance * ((limb * edge + corona) * SUN_DISC) * (1.0 - moon_cover(direction, sky.moon_direction, pixel));
}

// Near-side maria in the disc's frame: centre, radii, darkness.
const MARIA_COUNT: u32 = 14u;
const MARIA = array<vec4<f32>, 14>(
    vec4<f32>(-0.23, 0.54, 0.3, 0.26),
    vec4<f32>(-0.72, 0.2, 0.19, 0.42),
    vec4<f32>(-0.52, 0.02, 0.22, 0.28),
    vec4<f32>(-0.37, -0.17, 0.13, 0.11),
    vec4<f32>(0.27, 0.47, 0.18, 0.16),
    vec4<f32>(0.5, 0.15, 0.24, 0.19),
    vec4<f32>(0.8, 0.29, 0.09, 0.14),
    vec4<f32>(0.74, -0.15, 0.12, 0.24),
    vec4<f32>(0.54, -0.27, 0.1, 0.1),
    vec4<f32>(-0.27, -0.37, 0.2, 0.17),
    vec4<f32>(-0.57, -0.41, 0.1, 0.1),
    vec4<f32>(-0.02, 0.82, 0.45, 0.06),
    vec4<f32>(0.06, 0.24, 0.09, 0.07),
    vec4<f32>(0.03, 0.04, 0.07, 0.05),
);
const MARIA_DEPTH = array<f32, 14>(1.0, 0.82, 0.78, 0.7, 0.86, 0.95, 1.0, 0.82, 0.82, 0.76, 0.9, 0.72, 0.8, 0.6);
// Named craters: centre, radius, contrast.
const CRATERS = array<vec4<f32>, 8>(
    vec4<f32>(-0.14, -0.69, 0.025, 0.75),
    vec4<f32>(-0.34, 0.17, 0.027, 0.45),
    vec4<f32>(-0.61, 0.14, 0.016, 0.4),
    vec4<f32>(-0.67, 0.4, 0.014, 0.85),
    vec4<f32>(0.86, -0.15, 0.025, 0.3),
    vec4<f32>(0.7, 0.27, 0.012, 0.5),
    vec4<f32>(-0.1, 0.78, 0.025, -0.3),
    vec4<f32>(-0.93, -0.09, 0.04, -0.32),
);

fn moon_rays(p: vec2<f32>, centre: vec2<f32>, reach: f32, strength: f32, seed: f32) -> f32 {
    let v = p - centre;
    let d = length(v);
    let streak = smoothstep(0.3, 0.85, mx_noise_float3(vec3<f32>(normalize(v) * 3.9, seed)));
    return streak * exp(d / -reach) * smoothstep(0.02, 0.07, d) * strength;
}

fn moon_ejecta(p: vec2<f32>, scale: f32, share: f32, seed: f32) -> f32 {
    let grid = p * scale + seed;
    let cell = floor(grid);
    let hash = fract(sin(dot(cell, vec2<f32>(127.1, 311.7))) * 43758.5453);
    let size = fract(hash * 13.7 + 0.31);
    let centre = cell + fract(vec2<f32>(hash * 17.3, hash * 29.9)) * 0.6 + 0.2;
    let r = length(grid - centre) / (size * 0.25 + 0.08);
    return exp(r * r * -1.5) * (size * 0.22 + 0.08) * smoothstep(1.0 - share, 1.02 - share, hash);
}

// Albedo of the near side at disc position `p` (radius 1); `pixels` is the disc's diameter on screen.
fn moon_albedo(p: vec2<f32>, pixels: f32) -> vec3<f32> {
    let coarse = smoothstep(20.0, 70.0, pixels);
    let detail = smoothstep(60.0, 220.0, pixels);
    let warped = p
        + vec2<f32>(mx_noise_float2(p * 3.1), mx_noise_float2(p * 3.1 + 17.3)) * 0.09
        + vec2<f32>(mx_noise_float2(p * 8.3 + 3.7), mx_noise_float2(p * 8.3 + 9.1)) * 0.035;
    let shore = mx_noise_float2(p * 12.7 + 4.1) * 0.1;
    var dark = 0.0;
    for (var i = 0u; i < MARIA_COUNT; i++) {
        let m = MARIA[i];
        let e = length((warped - m.xy) / m.zw) + shore;
        dark = max(dark, smoothstep(1.08, 0.74, e) * MARIA_DEPTH[i]);
    }
    dark *= mx_noise_float2(p * 6.1 + 11.7) * 0.16 + 0.92;
    let mottle = mx_noise_float2(p * 8.3 + 1.3) * 0.09 * coarse + mx_noise_float2(p * 21.5 + 8.2) * 0.05 * detail;
    var albedo = (1.0 - dark * 0.44) * (mottle + 1.0);
    for (var i = 0u; i < 8u; i++) {
        let c = CRATERS[i];
        let d = length(p - c.xy);
        let near = d / c.z;
        let far = d / (c.z * 3.0);
        albedo += exp(-(near * near)) * c.w * coarse + exp(-(far * far)) * (c.w * 0.2);
    }
    albedo += moon_rays(p, vec2<f32>(-0.14, -0.69), 0.55, 0.26, 1.7) * (coarse * 0.5 + 0.5)
        + moon_rays(p, vec2<f32>(-0.34, 0.17), 0.22, 0.14, 5.3) * coarse;
    let craters = (moon_ejecta(p, 21.0, 0.22, 0.0) + moon_ejecta(p, 53.0, 0.15, 7.3)) * detail;
    return mix(vec3<f32>(1.0, 0.98, 0.95), vec3<f32>(0.93, 0.95, 1.0), dark) * (albedo + craters);
}

// The moon along `direction`: a sphere lit by the true sun, Lommel–Seeliger, earthshine and a tight halo.
fn moon_disc(direction: vec3<f32>, pixel: f32) -> vec3<f32> {
    let reach = DISC_RADIUS + MOON_HALO_WIDTH * 10.0;
    let offset = direction - sky.moon_direction;
    let angle2 = dot(offset, offset);
    if angle2 >= reach * reach {
        return vec3<f32>(0.0);
    }
    let angle = sqrt(angle2);
    let edge = smoothstep(-pixel * EDGE_PX, pixel * EDGE_PX, -(angle - DISC_RADIUS));
    var result = sky.moon_surface * sky.moon_glow * exp(max(angle - DISC_RADIUS, 0.0) / -MOON_HALO_WIDTH) * (1.0 - edge);
    if edge > 0.0 {
        let p = vec2<f32>(dot(offset, sky.moon_right), dot(offset, sky.moon_up)) / DISC_RADIUS;
        let z = sqrt(max(1.0 - dot(p, p), 0.0));
        let rough = mx_noise_float2(p * 9.3 + 2.2) * 0.03 + mx_noise_float2(p * 23.1) * 0.01;
        let lit = max(dot(vec3<f32>(p, z), sky.moon_sun) + rough, 0.0);
        let shade = lit / (lit + z + 1e-3) * 1.7 + lit * 0.15;
        let albedo = moon_albedo(p, (1.0 / pixel) * (DISC_RADIUS * 2.0));
        result += sky.moon_surface * albedo * (sky.earthshine * pow(z, 0.4) + shade * sky.moon_opposition) * edge;
    }
    return result;
}

// The moon for a blurred bake: a blob of the disc's energy, `spread` radians wide.
fn moon_glow(direction: vec3<f32>, spread: f32) -> vec3<f32> {
    let offset = direction - sky.moon_direction;
    let angle2 = dot(offset, offset);
    let energy = sky.moon_surface * sky.moon_opposition * sky.moon_lit * (0.8 * DISC_RADIUS * DISC_RADIUS / 2.0) / (spread * spread);
    return energy * exp(angle2 / (spread * spread * -2.0));
}

fn star_warp(u: f32) -> f32 {
    let a = abs(u);
    return u * ((1.0 - a) * (a * WARP_B + WARP_A) + 1.0);
}

fn star_warp_slope(u: f32) -> f32 {
    let a = abs(u);
    return a * (2.0 * (WARP_B - WARP_A)) - a * a * (3.0 * WARP_B) + (1.0 + WARP_A);
}

// One star of a cell (its texel: the place in the cell, magnitude, B−V), if it reaches this direction.
fn star_of(texel: vec4<f32>, local: vec2<f32>, uv: vec2<f32>, direction: vec3<f32>, pixel: f32) -> vec3<f32> {
    let n = STAR_CELLS;
    let limit = sky.star_limit;
    if texel.z >= limit + 0.5 {
        return vec3<f32>(0.0);
    }
    let d = (local - texel.xy) * (2.0 / n) / vec2<f32>(star_warp_slope(uv.x), star_warp_slope(uv.y));
    let u2 = uv.x * uv.x;
    let v2 = uv.y * uv.y;
    let c = u2 + v2 + 1.0;
    let angle2 = ((v2 + 1.0) * d.x * d.x + (u2 + 1.0) * d.y * d.y - uv.x * uv.y * d.x * d.y * 2.0) / (c * c);
    if angle2 >= STAR_MARGIN * STAR_MARGIN {
        return vec3<f32>(0.0);
    }
    let flux = exp2(texel.z * -STAR_FLUX_EXPONENT) * STAR_PEAK * smoothstep(limit + 0.5, limit - 0.5, texel.z);
    let spread = pixel * SPREAD_PX * max(1.0, (1.5 - texel.z) * BRIGHT_SPREAD + 1.0);
    let fall = angle2 / (STAR_MARGIN * STAR_MARGIN);
    let image = exp(angle2 / (spread * spread * -2.0)) * (1.0 - fall * fall);
    let air = 1.0 - smoothstep(0.0, 0.5, direction.y);
    // Seeded by the star's magnitude, not by where its cell has it: a star across a face's edge is in a cell of each
    // face, and both parts flicker as one.
    let hue = texel.w - select(0.0, STAR_MORE, texel.w >= STAR_MORE_FROM);
    let seed = fract(texel.z * vec2<f32>(71.37, 23.91) + vec2<f32>(0.37, 0.11));
    let flicker = sin(sky.time * ((seed.x * 2.3 + 1.9) * (2.0 * PI)) + seed.y * 40.0) * 0.6
        + sin(sky.time * ((seed.y * 2.9 + 3.1) * (2.0 * PI)) + seed.x * 57.0) * 0.4;
    let twinkle = max(flicker * mix(TWINKLE_ZENITH, TWINKLE_HORIZON, air * air) + 1.0, 0.15);
    let ramp_u = (hue - COLOR_INDEX_LOW) / (COLOR_INDEX_HIGH - COLOR_INDEX_LOW);
    let tint = textureSampleLevel(star_ramp, linear_clamp, vec2<f32>(ramp_u, 0.5), 0.0).rgb;
    let color = mix(tint, vec3<f32>(1.0), smoothstep(COLOR_FADE.x, COLOR_FADE.y, texel.z) * 0.6);
    return color * flux * image * twinkle;
}

// One cell's stars. Stars too close for a cell each share one, a layer apart (`stars::STAR_LAYERS`), each flagging the
// next by its B−V raised by `STAR_MORE`.
fn star_in_cell(face: f32, cell: vec2<f32>, local: vec2<f32>, row: f32, uv: vec2<f32>, direction: vec3<f32>, pixel: f32) -> vec3<f32> {
    let n = STAR_CELLS;
    let at = vec2<i32>(vec2<f32>(face * (n + 1.0) + cell.x, cell.y + row));
    var texel = textureLoad(star_grid, at, 0);
    var result = star_of(texel, local, uv, direction, pixel);
    for (var layer = 1; layer < STAR_LAYERS && texel.w >= STAR_MORE_FROM; layer++) {
        texel = textureLoad(star_grid, at + vec2<i32>(0, layer * STAR_LAYER_ROWS), 0);
        result += star_of(texel, local, uv, direction, pixel);
    }
    return result;
}

// Radiance of the stars around one direction (`StarField.radiance`).
fn stars(galactic: vec3<f32>, direction: vec3<f32>, pixel: f32) -> vec3<f32> {
    let n = STAR_CELLS;
    let g = galactic;
    let a = abs(g);
    let x_major = a.x >= a.y && a.x >= a.z;
    let y_major = !x_major && a.y >= a.z;
    let major = select(select(a.z, a.y, y_major), a.x, x_major);
    let face = select(
        select(select(5.0, 4.0, g.z >= 0.0), select(3.0, 2.0, g.y >= 0.0), y_major),
        select(1.0, 0.0, g.x >= 0.0),
        x_major);
    let uv = select(select(g.xy, g.zx, y_major), g.yz, x_major) / major;
    let grid = (vec2<f32>(star_warp(uv.x), star_warp(uv.y)) + 1.0) * (n / 2.0);
    // Four grids (`stars::STAR_GRIDS`): whole cells, cells shifted half a cell along both axes, along the first and
    // along the second. A star is in whichever leaves its disc clear of the cell's edges.
    let whole = min(floor(grid), vec2<f32>(n - 1.0));
    let in_whole = grid - whole;
    var result = star_in_cell(face, whole, in_whole, 0.0, uv, direction, pixel);
    let shifted = grid + 0.5;
    let half = floor(shifted);
    let in_half = shifted - half;
    result += star_in_cell(face, half, in_half, n + 1.0, uv, direction, pixel);
    result += star_in_cell(face, vec2<f32>(half.x, whole.y), vec2<f32>(in_half.x, in_whole.y), (n + 1.0) * 2.0, uv, direction, pixel);
    result += star_in_cell(face, vec2<f32>(whole.x, half.y), vec2<f32>(in_whole.x, in_half.y), (n + 1.0) * 3.0, uv, direction, pixel);
    return result * sky.star_gain;
}

// Relative radiance of the Milky Way along a galactic direction at a mip `level`.
fn milky_way(g: vec3<f32>, level: f32) -> vec3<f32> {
    let at = vec2<f32>(atan2(g.y, g.x) / (2.0 * PI) + 0.5, g.z * 0.5 + 0.5);
    let encoded = textureSampleLevel(milky_way_map, milky_way_sampler, at, level).rgb;
    return encoded * encoded * MILKY_WAY_ENCODING;
}

fn to_galactic(direction: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(sky.galactic_x, direction), dot(sky.galactic_y, direction), dot(sky.galactic_z, direction));
}

// Radians a pixel spans along `direction`: the centre's, shrinking off axis.
fn pixel_along(direction: vec3<f32>) -> f32 {
    let facing = max(dot(direction, sky.forward), 0.2);
    return sky.pixel_angle * (facing * sqrt(facing));
}

// Radiance above the atmosphere along a world direction: sun disc, moon, stars, Milky Way (`radiance`).
fn celestial_radiance(direction: vec3<f32>) -> vec3<f32> {
    let pixel = pixel_along(direction);
    var result = sun_disc(direction, pixel) + moon_disc(direction, pixel);
    if sky.night > 0.0 && direction.y > sky.star_horizon {
        let galactic = to_galactic(direction);
        let s = stars(galactic, direction, pixel);
        // `sky.haze.z` is the host's night sky brightness: it dims the dome's flat glow only, not the Milky Way, the
        // stars, or the environment bake's glow (`celestial_diffuse_radiance`).
        result += (s + (milky_way(galactic, 0.0) + NIGHT_GLOW * sky.haze.z) * sky.milky_way) * (1.0 - moon_cover(direction, sky.moon_direction, pixel));
    }
    return result;
}

// The same without the sun disc and stars: what a blurred reflection bake holds (`diffuseRadiance`).
fn celestial_diffuse_radiance(direction: vec3<f32>) -> vec3<f32> {
    var result = moon_glow(direction, sky.bake_spread);
    if sky.milky_way > 0.0 {
        result += (milky_way(to_galactic(direction), sky.bake_level) + NIGHT_GLOW) * sky.milky_way;
    }
    return result * smoothstep(-0.02, 0.02, direction.y);
}
