// The wave field as the surface and the passes beside it read it (group 2): the `displacement` and `heightAt` reads of
// the web game's `ocean/waves/field.ts`, and the sea around hulls of `ocean/waves/hullSea.ts` (`HullSea`: `weight`,
// `weightGradient`, `blend`, `height`, `surface`, `displace`; field.ts's `longWaves` and `hullSurface`). Near each hull
// the first cascade's long waves (low-passed by its mip chain to the split level) give way to combat's two sines at the
// simulation's clock, the shorter waves, foam and wake riding on top; further out the field is drawn as it is. With the
// sea drawn everywhere (`HullSeaUniforms::set_everywhere`, the game's realistic sea) the weight is 1 at every point: the
// sines the hulls ride are the open sea's long swell too, spread into a short-crested swell away from the hulls.
#define_import_path naval_ocean::waves

#import naval_ocean::noise::{value_noise, noise_hash, noise_seed}

struct WaveUniforms {
    tile: vec4<f32>,
    texels: vec4<f32>,
    longest: vec4<f32>,
    slopes: vec4<f32>,
    slope_lengths: vec4<f32>,
    gates: vec4<f32>,
    gust_shares: vec4<f32>,
    groups: array<vec4<f32>, 3>,
    tail: vec4<f32>,
    info: vec4<f32>,
    wind: vec4<f32>,
    gusts: vec4<f32>,
    sea: vec4<f32>,
    // Per cascade: share of its slope in the short waves a wake's slick stills.
    slick_shares: vec4<f32>,
    // The coast (`naval_ocean::shore`, `WaveUniforms::shore_*`): the heights map's and the exposure map's frames (x, z of
    // the first texel's corner, texture coordinate per metre); per cascade and (w) the swell the open sea's significant
    // height; (1 with a coast, unused, 1 / the swell's shelter length in km, the surf's phase); per fetch of the table
    // (`shore::table_fetch`) each cascade's share of its amplitude kept.
    shore_heights: vec4<f32>,
    shore_exposure: vec4<f32>,
    shore_waves: vec4<f32>,
    shore: vec4<f32>,
    shore_fetch: array<vec4<f32>, 16>,
}

// `naval_ocean::hull_sea::HullSeaUniforms`: per hull (centre x, z, bow axis x, z) and (half length, inner, outer,
// contact); per component (kx, kz, amplitude, phase about `origin`), combat's then the open sea's spread; (origin x, z,
// the split mip level, 1 while the first cascade's long waves are replaced); (hulls coupled, 1 while the sea is drawn
// everywhere, each combat sine's share of the open sea's swell); (how far the hulls are let go: 1 while the simulation
// runs faster than the sea is drawn, its sines' energy all the spread's, on the sea's own clock).
struct HullSea {
    slots: array<vec4<f32>, 32>,
    waves: array<vec4<f32>, 2>,
    swell: array<vec4<f32>, 16>,
    origin: vec4<f32>,
    count: vec4<f32>,
    clock: vec4<f32>,
}

@group(2) @binding(0) var field_displacement: texture_2d_array<f32>;
@group(2) @binding(1) var field_derivatives: texture_2d_array<f32>;
@group(2) @binding(2) var field_extras: texture_2d_array<f32>;
@group(2) @binding(3) var field_sampler: sampler;
@group(2) @binding(4) var<uniform> waves: WaveUniforms;
@group(2) @binding(5) var<uniform> hull_sea: HullSea;
// The coast (`naval_ocean::shore::ShoreTextures`): the land's heights (m, negative under water) and the open water
// upwind of the wind sea and of the swell (km), read with a clamping linear sampler.
@group(2) @binding(6) var shore_heights: texture_2d<f32>;
@group(2) @binding(7) var shore_exposure: texture_2d<f32>;
@group(2) @binding(8) var shore_sampler: sampler;

const PI: f32 = 3.141592653589793;
const GROUP_SEED: u32 = 101u;
const GROUP_EDGE: f32 = 0.06;
// How long, relative to its foam's own lifetime, a patch can be read as old.
const OLDEST: f32 = 6.0;
// `INVERSION_STEPS`, `INVERSION_DAMPING`: the choppy map's fixed-point inversion, damped after the first step.
const INVERSION_STEPS: i32 = 6;
const INVERSION_DAMPING: f32 = 0.7;

fn field_uv(xz: vec2<f32>, i: i32) -> vec2<f32> {
    return xz / waves.tile[i] + 0.5 / waves.info.z;
}

// The two coarser wind-wave cascades drawn without their 1024 m and 181 m repeats, which sun glitter reveals from
// above. Fixing only the middle cascade leaves recognizable patches from the larger field. A rotated
// triangular grid chooses three independently shifted copies, blended with squared weights summing to one: slopes,
// strain and displacement keep their energy; positive quantities (foam, bubbles and per-copy unresolved variance)
// take the squared weights, keeping their means. Explicit texture gradients use the pixel's footprint, never a
// neighbour's random shift at a cell boundary. The first cascade's hull-sea lowpass uses these same copies; the
// finest ripples keep their original reads.
// Stochastic tiling: Heitz & Neyret 2018.
const SCATTER_CELL: f32 = 0.93;
const SCATTER_SEED: u32 = 211u;
const SCATTER_FLOOR: f32 = 0.02;

// Where cascade `i`'s three copies are read at grid point `xz`, and their weights (Σ a_k² = 1; a cascade read as it is
// takes its first copy alone).
struct Scatter {
    uv_a: vec2<f32>,
    uv_b: vec2<f32>,
    uv_c: vec2<f32>,
    weights: vec3<f32>,
}

// A triangle grid vertex's shift of the tile (uv), for cascade `i`.
fn scatter_shift(vertex: vec2<f32>, i: i32) -> vec2<f32> {
    let x = i32(vertex.x);
    let y = i32(vertex.y);
    let seed = noise_seed(SCATTER_SEED + u32(i));
    return vec2<f32>(noise_hash(x, y, seed), noise_hash(x, y, seed ^ 0x5bd1e995u));
}

fn scatter(xz: vec2<f32>, i: i32) -> Scatter {
    let uv = field_uv(xz, i);
    if i >= 2 {
        return Scatter(uv, uv, uv, vec3<f32>(1.0, 0.0, 0.0));
    }
    let p = vec2<f32>(xz.x * 0.8 - xz.y * 0.6, xz.x * 0.6 + xz.y * 0.8) / (waves.tile[i] * SCATTER_CELL);
    // Skewed onto the triangle grid: the point's triangle, its three vertices and barycentric weights.
    let skewed = vec2<f32>(p.x - p.y * 0.57735027, p.y * 1.15470054);
    let base = floor(skewed);
    let f = skewed - base;
    let z = 1.0 - f.x - f.y;
    var barycentric = vec3<f32>(-z, 1.0 - f.y, 1.0 - f.x);
    var first = base + vec2<f32>(1.0, 1.0);
    if z > 0.0 {
        barycentric = vec3<f32>(z, f.y, f.x);
        first = base;
    }
    let second = base + select(vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), z > 0.0);
    let third = base + select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), z > 0.0);
    let squared = barycentric * barycentric;
    let raised = squared * barycentric;
    let kept = select(raised, vec3<f32>(0.0), raised < vec3<f32>(max(raised.x, max(raised.y, raised.z)) * SCATTER_FLOOR));
    return Scatter(uv + scatter_shift(first, i), uv + scatter_shift(second, i), uv + scatter_shift(third, i),
        kept / length(kept));
}

// The three copies' reads (a copy with no weight is not read and reads 0).
struct Copies {
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
}

fn copies_level(map: texture_2d_array<f32>, at: Scatter, i: i32, level: f32) -> Copies {
    var read = Copies(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    if at.weights.x > 0.0 {
        read.a = textureSampleLevel(map, field_sampler, at.uv_a, i, level);
    }
    if at.weights.y > 0.0 {
        read.b = textureSampleLevel(map, field_sampler, at.uv_b, i, level);
    }
    if at.weights.z > 0.0 {
        read.c = textureSampleLevel(map, field_sampler, at.uv_c, i, level);
    }
    return read;
}

// The same with a pixel's footprint (`dx`, `dy`: its extent in uv), as an implicit-derivative read filters it.
fn copies_grad(map: texture_2d_array<f32>, at: Scatter, i: i32, dx: vec2<f32>, dy: vec2<f32>) -> Copies {
    var read = Copies(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    if at.weights.x > 0.0 {
        read.a = textureSampleGrad(map, field_sampler, at.uv_a, i, dx, dy);
    }
    if at.weights.y > 0.0 {
        read.b = textureSampleGrad(map, field_sampler, at.uv_b, i, dx, dy);
    }
    if at.weights.z > 0.0 {
        read.c = textureSampleGrad(map, field_sampler, at.uv_c, i, dx, dy);
    }
    return read;
}

// A zero-mean field's copies mixed, its variance kept.
fn mix_zero(read: Copies, at: Scatter) -> vec4<f32> {
    return read.a * at.weights.x + read.b * at.weights.y + read.c * at.weights.z;
}

// A positive field's copies mixed (foam, bubbles, second moments), its mean kept.
fn mix_mean(read: Copies, at: Scatter) -> vec4<f32> {
    let shares = at.weights * at.weights;
    return read.a * shares.x + read.b * shares.y + read.c * shares.z;
}

// The slope variance within a pixel (each copy's second moment `extras.z` less its mean slope squared), mixed as a mean:
// the copies are independent.
fn mix_variance(extras: Copies, derivatives: Copies, at: Scatter) -> f32 {
    let shares = at.weights * at.weights;
    return max(extras.a.z - dot(derivatives.a.xy, derivatives.a.xy), 0.0) * shares.x
        + max(extras.b.z - dot(derivatives.b.xy, derivatives.b.xy), 0.0) * shares.y
        + max(extras.c.z - dot(derivatives.c.xy, derivatives.c.xy), 0.0) * shares.z;
}

// How much of cascade `i`'s foam (`foam` and `bubbles`, from one read) at grid point `xz` its groups show: the groups
// where the patch was born (`breakingGroup` of the web game's `ocean/waves/groups.ts`). The surface draws whitecaps through it,
// and the spindrift rises from what it shows.
fn breaking_group(xz: vec2<f32>, foam: f32, bubbles: f32, i: i32) -> f32 {
    let group = waves.groups[i];
    let wind = waves.wind.xy;
    let fresh = clamp(bubbles / max(foam, 1e-5), exp(-OLDEST), 1.0);
    let born = waves.wind.z + log(fresh) * group.w;
    let along = dot(xz, wind) - born * group.z;
    let across = dot(xz, vec2<f32>(-wind.y, wind.x));
    let cut = waves.wind.w;
    return smoothstep(cut - GROUP_EDGE, cut + GROUP_EDGE, value_noise(vec2<f32>(along, across) * group.xy, GROUP_SEED + u32(i)));
}

// What a coast leaves of the waves at a world position (`naval_ocean::shore`): per cascade (xyz) and for the swell (w)
// the share of the open sea's amplitude drawn there; the water's depth (m) and the significant height of the waves
// arriving there before the depth breaks them (m).
struct Shore {
    scales: vec4<f32>,
    depth: f32, // signed still-water depth (negative above the waterline)
    arriving: f32,
}

// `shore::BREAKING`, and the depth and open water a point off the coast's maps has.
const SHORE_BREAKING: f32 = 0.42;
const SHORE_DEEP: f32 = 1.0e4;
const SHORE_OPEN_KM: f32 = 100.0;
// `shore::FETCH_LEAST_M` in km, and the table's last entry (`shore::FETCH_TABLE` − 1).
const SHORE_FETCH_LEAST_KM: f32 = 0.05;
const SHORE_FETCH_LAST: f32 = 15.0;
// The surf's bores run in at √(g·d) (g, m/s²) over a seabed no gentler than 1 in 200; its breakers run this far
// along the shore before they tear (m).
const SURF_GRAVITY: f32 = 9.81;
const SURF_GENTLEST: f32 = 0.005;
const SURF_TEAR: f32 = 22.0;
// The broken water left between the lines, as a share of a line's.
const SURF_RESIDUE: f32 = 0.3;
// A line's mean over its period (smoothstep(0.35, 0.85, ½ + ½·cos φ)) and the tear's over the noise (about), drawn
// where the pixel is too large for them.
const SURF_LINE_MEAN: f32 = 0.434;
const SURF_TORN_MEAN: f32 = 0.65;

fn shore_inside(uv: vec2<f32>) -> bool {
    return all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
}

fn shore_height(uv: vec2<f32>) -> vec2<f32> {
    let height = textureSampleLevel(shore_heights, shore_sampler, uv, 0.0).rg;
    return vec2<f32>(height.x / max(height.y, 0.5), height.y);
}

fn shore_at(xz: vec2<f32>) -> Shore {
    if waves.shore.x < 0.5 {
        return Shore(vec4<f32>(1.0), SHORE_DEEP, 0.0);
    }
    let heights_uv = (xz - waves.shore_heights.xy) * waves.shore_heights.zw;
    var depth = SHORE_DEEP;
    if shore_inside(heights_uv) {
        let height = shore_height(heights_uv);
        depth = select(SHORE_DEEP, -height.x, height.y >= 0.5);
    }
    let exposure_uv = (xz - waves.shore_exposure.xy) * waves.shore_exposure.zw;
    var fetch = vec2<f32>(SHORE_OPEN_KM);
    if shore_inside(exposure_uv) {
        fetch = textureSampleLevel(shore_exposure, shore_sampler, exposure_uv, 0.0).rg;
    }
    // The wind sea: in a lee its peak is the fetch's, and the waves longer than that are not yet raised.
    let entry = clamp(log(max(fetch.x, 1e-4) / SHORE_FETCH_LEAST_KM) / log(SHORE_OPEN_KM / SHORE_FETCH_LEAST_KM)
        * SHORE_FETCH_LAST, 0.0, SHORE_FETCH_LAST);
    let below = min(u32(entry), u32(SHORE_FETCH_LAST) - 1u);
    var scales = mix(waves.shore_fetch[below], waves.shore_fetch[below + 1u], entry - f32(below));
    // The swell: none in the land's shadow, filling back in with open water.
    let fill = 1.0 - exp(-max(fetch.y, 0.0) * waves.shore.z);
    scales.w = min(fill / (1.0 - exp(-SHORE_OPEN_KM * waves.shore.z)), 1.0);
    let arriving = length(waves.shore_waves * scales);
    // Shoaling water: no wave higher than the breaking share of its depth.
    scales = min(scales, SHORE_BREAKING * max(depth, 0.0) / max(waves.shore_waves, vec4<f32>(1e-3)));
    return Shore(scales, depth, arriving);
}

// The share of the open sea's whitecaps a coast leaves at `shore`: the energy its cascades keep there, as their foam
// keeps the square of their amplitude. 1 without a coast.
fn shore_whitecaps(shore: Shore) -> f32 {
    let open = dot(waves.shore_waves.xyz, waves.shore_waves.xyz);
    if waves.shore.x < 0.5 || open <= 0.0 {
        return 1.0;
    }
    let kept = shore.scales.xyz * waves.shore_waves.xyz;
    return dot(kept, kept) / open;
}

// The seabed's rise over its run at world position `xz`, across a sample of the coast's height grid either way.
fn shore_slope(xz: vec2<f32>, bed: f32) -> f32 {
    let size = vec2<f32>(textureDimensions(shore_heights));
    let uv = (xz - waves.shore_heights.xy) * waves.shore_heights.zw;
    let du = vec2<f32>(1.0 / size.x, 0.0);
    let dv = vec2<f32>(0.0, 1.0 / size.y);
    let right = shore_height(uv + du);
    let left = shore_height(uv - du);
    let next = shore_height(uv + dv);
    let prev = shore_height(uv - dv);
    let forward_valid = vec2<f32>(right.y, next.y) >= vec2<f32>(0.5);
    let backward_valid = vec2<f32>(left.y, prev.y) >= vec2<f32>(0.5);
    let forward = select(vec2<f32>(bed), vec2<f32>(right.x, next.x), forward_valid);
    let backward = select(vec2<f32>(bed), vec2<f32>(left.x, prev.x), backward_valid);
    // Use a one-sided slope at the edge of known bed coverage.
    let steps = vec2<f32>(select(1.0, 2.0, forward_valid.x && backward_valid.x),
        select(1.0, 2.0, forward_valid.y && backward_valid.y));
    return length((forward - backward) * size * waves.shore_heights.zw / steps);
}

// The surf at world position `xz` (x: its cover 0 … 1, y: the fresh white water's share of it), for a pixel `across`
// metres toward the beach and `along` it: where the water is shallower than the arriving waves break in, the broken
// waves run in to the beach as bores at √(g·d), one a swell period. Over a seabed rising at s a bore takes
// 2·√d / (s·√g) from depth d to the waterline, so its lines lie a shallow-water wavelength T·√(g·d) apart and crowd
// together as the water shoals: a gentle beach holds several at once, a steep one less than one, a white band that runs
// in and drains. Each line is torn along its length its own way, with broken water left between them; lines closer
// than a few pixels, and tears smaller than one, are drawn as their mean. None outside the surf zone.
fn surf(xz: vec2<f32>, shore: Shore, across: f32, along: f32) -> vec2<f32> {
    let breaking = shore.arriving / SHORE_BREAKING;
    let frequency = waves.shore.y;
    if breaking < 0.1 || shore.depth <= 0.0 || shore.depth >= breaking || frequency <= 0.0 {
        return vec2<f32>(0.0);
    }
    let depth = shore.depth;
    let slope = clamp(shore_slope(xz, -depth), SURF_GENTLEST, 1.0);
    let phase = waves.shore.w + frequency * 2.0 * sqrt(depth) / (slope * sqrt(SURF_GRAVITY));
    let spacing = 6.2831853 * sqrt(SURF_GRAVITY * depth) / frequency;
    let resolved = smoothstep(2.0, 6.0, spacing / max(across, 1e-3));
    let line = mix(SURF_LINE_MEAN, smoothstep(0.35, 0.85, 0.5 + 0.5 * cos(phase)), resolved);
    let which = floor(phase / 6.2831853 + 0.5);
    let tear = value_noise(xz / SURF_TEAR + vec2<f32>(which * 7.31, which * 3.17), 613u);
    let tear_shown = (1.0 - smoothstep(SURF_TEAR * 0.25, SURF_TEAR * 0.6, along)) * resolved;
    let fresh = line * mix(SURF_TORN_MEAN, smoothstep(0.2, 0.5, tear), tear_shown);
    let zone = smoothstep(1.0, 0.75, depth / breaking);
    return vec2<f32>(mix(SURF_RESIDUE, 1.0, fresh), fresh) * zone;
}

// Anchor breaker foam to the drawn surface, including horizontal chop.
fn surf_at(xz: vec2<f32>, across: f32, along: f32) -> vec2<f32> {
    if waves.shore.x < 0.5 {
        return vec2<f32>(0.0);
    }
    return surf(xz, shore_at(xz), across, along);
}

// The first cascade low-passed to the hull sea's split and never finer than mip `fine`: the long waves hulls replace
// (`longWaves` at a level), 0 when none are replaced.
fn long_wave(map: texture_2d_array<f32>, xz: vec2<f32>, fine: f32) -> vec4<f32> {
    let level = clamp(max(fine, hull_sea.origin.z), 0.0, waves.info.x);
    let at = scatter(xz, 0);
    return mix_zero(copies_level(map, at, 0, level), at) * hull_sea.origin.w;
}

// Whether combat's sea is drawn everywhere (weight 1 at every point).
fn hull_sea_everywhere() -> bool {
    return hull_sea.count.y > 0.5;
}

// Coupling weight at a world position (`HullSea.weight`): each hull's smoothstep of the distance from its centre line,
// joined as 1 − Π(1 − wᵢ); 1 with the sea drawn everywhere.
fn hull_weight(xz: vec2<f32>) -> f32 {
    if hull_sea_everywhere() {
        return 1.0;
    }
    var keep = 1.0;
    let count = i32(hull_sea.count.x);
    for (var i = 0; i < count; i++) {
        let a = hull_sea.slots[i * 2];
        let b = hull_sea.slots[i * 2 + 1];
        let relative = xz - a.xy;
        let t = clamp(dot(relative, a.zw), -b.x, b.x);
        let distance = length(relative - a.zw * t);
        keep *= 1.0 - b.w * (1.0 - smoothstep(b.y, b.z, distance));
    }
    return 1.0 - keep;
}

// (weight, ∂w/∂x, ∂w/∂z) at a world position (`HullSea.weightGradient`).
fn hull_weight_gradient(xz: vec2<f32>) -> vec3<f32> {
    if hull_sea_everywhere() {
        return vec3<f32>(1.0, 0.0, 0.0);
    }
    var keep = 1.0;
    var gradient = vec2<f32>(0.0);
    let count = i32(hull_sea.count.x);
    for (var i = 0; i < count; i++) {
        let a = hull_sea.slots[i * 2];
        let b = hull_sea.slots[i * 2 + 1];
        let relative = xz - a.xy;
        let t = clamp(dot(relative, a.zw), -b.x, b.x);
        let offset = relative - a.zw * t;
        let distance = length(offset);
        let span = max(b.z - b.y, 1e-3);
        let s = clamp((distance - b.y) / span, 0.0, 1.0);
        let w = b.w * (1.0 - s * s * (3.0 - 2.0 * s));
        let rate = b.w * s * (1.0 - s) * -6.0 / span / max(distance, 1e-3);
        gradient = gradient * (1.0 - w) - offset * rate * keep;
        keep *= 1.0 - w;
    }
    return vec3<f32>(1.0 - keep, -gradient);
}

// `HullSea.blend`: height = field + lowpass·(the field's long waves) + sea·(combat's sea), with their gradients.
struct HullBlend {
    lowpass: f32,
    sea: f32,
    lowpass_gradient: vec2<f32>,
    sea_gradient: vec2<f32>,
}

fn hull_blend(w: f32, gradient: vec2<f32>) -> HullBlend {
    let n = inverseSqrt(1.0 - w * 2.0 + w * w * 2.0);
    let dn = (1.0 - w * 2.0) * n * n * n;
    return HullBlend(n * (1.0 - w) - 1.0, n * w, gradient * (dn * (1.0 - w) - n), gradient * (dn * w + n));
}

// The open sea's swell. Drawn everywhere, combat's two sines would cross the whole sea as one even corrugation, ruled
// straight to the horizon, which from a few hundred metres up reads as a pattern laid on the water. Away from the hulls
// each sine is one of a spread of components (`HullSea.swell`, `naval_ocean::hull_sea::open_sea_swell`: directions
// within about 46° of its own, lengths within 0.7–1.4 times, an equal share of its energy each) whose crests interfere
// as a short-crested sea's do: crests a wavelength or two long, sets that build and fade, nothing ruled or repeating.
// Near the hulls (`swell_freedom`) the sines take their energy back, so every hull rides the sea the simulation moves
// her on; in between the variance holds (`swell_height`).

// Combat sine `j`'s amplitude scale `free` away from the hulls: 1 at a hull, its share of the spread's energy away.
fn swell_keep(j: i32, free: f32) -> f32 {
    let share = select(hull_sea.count.w, hull_sea.count.z, j == 0) * (1.0 - hull_sea.clock.x);
    return sqrt(max(1.0 - free * free * (1.0 - share), 0.0));
}

// How far a world position lies from the hulls for the open sea's swell, while the sea is drawn everywhere (else 0):
// Π(1 − wᵢ) over each coupled hull's smoothstep of the distance from its centre line, whatever her contact with the
// surface (a boat awash rides combat's sea too). Near the hulls the swell is combat's alone; hulls let go are free.
fn swell_freedom(at: vec2<f32>) -> f32 {
    if !hull_sea_everywhere() {
        return 0.0;
    }
    var free = 1.0;
    let count = i32(hull_sea.count.x);
    for (var i = 0; i < count; i++) {
        let a = hull_sea.slots[i * 2];
        let b = hull_sea.slots[i * 2 + 1];
        let relative = at - a.xy;
        let t = clamp(dot(relative, a.zw), -b.x, b.x);
        free *= smoothstep(b.y, b.z, length(relative - a.zw * t));
    }
    return mix(free, 1.0, hull_sea.clock.x);
}

// One component's height at `relative` (about the origin), scaled by `scale`, for a vertex `spacing` metres from its
// neighbours (it fades out between four and two vertices per wave, as the cascades do); no spacing (≤ 0) fades nothing.
fn swell_component_height(wave: vec4<f32>, relative: vec2<f32>, scale: f32, spacing: f32) -> f32 {
    let k = length(wave.xy);
    let fade = select(1.0, 1.0 - smoothstep(PI / 2.0, PI, spacing * k), spacing > 0.0);
    return wave.z * scale * fade * sin(dot(wave.xy, relative) + wave.w);
}

// Combat's sea height at a world position (`HullSea.height`) for a vertex `spacing` metres from its neighbours. Away
// from the hulls, the open sea's swell (`swell_keep` and the spread), `shelter` of it left by a coast (`Shore.scales.w`;
// the hulls ride the simulation's sea, which knows none).
fn hull_height(at: vec2<f32>, spacing: f32, shelter: f32) -> f32 {
    let relative = at - hull_sea.origin.xy;
    let free = swell_freedom(at);
    var height = 0.0;
    for (var j = 0; j < 2; j++) {
        height += swell_component_height(hull_sea.waves[j], relative, swell_keep(j, free), spacing);
    }
    if free > 0.0 {
        for (var i = 0; i < 16; i++) {
            height += swell_component_height(hull_sea.swell[i], relative, free, spacing);
        }
    }
    return height * mix(1.0, shelter, free);
}

// One component's (height, ∂h/∂x, ∂h/∂z, slope variance filtered away) at `relative`, scaled by `scale`, seen by a
// pixel `footprint` metres wide: the slope attenuated as the mip chain's box would, the variance it loses turned to
// roughness.
fn swell_component_surface(wave: vec4<f32>, relative: vec2<f32>, scale: f32, footprint: f32) -> vec4<f32> {
    let k = length(wave.xy);
    let amplitude = wave.z * scale;
    let phase = dot(wave.xy, relative) + wave.w;
    let resolved = exp(pow(footprint * k, 2.0) / -8.0);
    let steepness = amplitude * k;
    return vec4<f32>(amplitude * sin(phase), wave.xy * (amplitude * cos(phase) * resolved),
        steepness * steepness * 0.5 * (1.0 - resolved * resolved));
}

// (height, ∂h/∂x, ∂h/∂z, slope variance filtered away) of combat's sea seen by a pixel `footprint` metres wide
// (`HullSea.surface`), away from the hulls the open sea's swell; the freedom's gradient is left out (the energy moves
// between components over a hull length or more, a few hundredths of the waves' own slope).
fn hull_sea_surface(at: vec2<f32>, footprint: f32, shelter: f32) -> vec4<f32> {
    let relative = at - hull_sea.origin.xy;
    let free = swell_freedom(at);
    var result = vec4<f32>(0.0);
    for (var j = 0; j < 2; j++) {
        result += swell_component_surface(hull_sea.waves[j], relative, swell_keep(j, free), footprint);
    }
    if free > 0.0 {
        for (var i = 0; i < 16; i++) {
            result += swell_component_surface(hull_sea.swell[i], relative, free, footprint);
        }
    }
    let kept = mix(1.0, shelter, free);
    return result * vec4<f32>(kept, kept, kept, kept * kept);
}

// Whether any hull is coupled (`HullSea.active`), or the sea drawn everywhere.
fn hull_sea_active() -> bool {
    return hull_sea.count.x > 0.5 || hull_sea_everywhere();
}

// World displacement of grid point `xz` whose vertex spacing is `spacing` (`displacement`): each cascade at the mip
// whose texels match the spacing, fading out between four and two vertices per its longest wave, then near hulls the
// long waves blended into combat's sea where the vertex lands (`HullSea.displace`); near a coast what it leaves of each.
fn displacement(xz: vec2<f32>, spacing: f32) -> vec3<f32> {
    let shore = shore_at(xz);
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < i32(waves.info.y); i++) {
        let level = clamp(log2(spacing * waves.texels[i]), 0.0, waves.info.x);
        let fade = 1.0 - smoothstep(waves.longest[i] * 0.25, waves.longest[i] * 0.5, spacing);
        let at = scatter(xz, i);
        sum += mix_zero(copies_level(field_displacement, at, i, level), at).xyz * (fade * shore.scales[i]);
    }
    if !hull_sea_active() {
        return sum;
    }
    let long_fade = 1.0 - smoothstep(waves.longest[0] * 0.25, waves.longest[0] * 0.5, spacing);
    let long = long_wave(field_displacement, xz, log2(spacing * waves.texels[0])).xyz * (long_fade * shore.scales.x);
    let blend = hull_blend(hull_weight(xz), vec2<f32>(0.0));
    let moved = sum + long * blend.lowpass;
    return vec3<f32>(moved.x, moved.y + blend.sea * hull_height(xz + moved.xz, spacing, shore.scales.w), moved.z);
}

// The hull sea's share of a pixel's surface (`hullSurface`): grid slope and strain to add with the first cascade's,
// world slope to add after the displaced-surface transform, and combat's unresolved slope variance. `at` is where the
// point is drawn; `footprint` the pixel's (m).
struct HullSurface {
    slope: vec2<f32>,
    strain: vec3<f32>,
    world: vec2<f32>,
    variance: f32,
}

fn hull_surface(xz: vec2<f32>, at: vec2<f32>, footprint: f32, shore: Shore) -> HullSurface {
    let weight = hull_weight_gradient(xz);
    let blend = hull_blend(weight.x, weight.yz);
    let level = log2(footprint * waves.texels[0]);
    let derivatives = long_wave(field_derivatives, xz, level) * shore.scales.x;
    let extras = long_wave(field_extras, xz, level) * shore.scales.x;
    let height = long_wave(field_displacement, xz, level).y * shore.scales.x;
    let sea = hull_sea_surface(at, footprint, shore.scales.w);
    return HullSurface(
        derivatives.xy * blend.lowpass,
        vec3<f32>(derivatives.zw, extras.x) * blend.lowpass,
        sea.yz * blend.sea + blend.lowpass_gradient * height + blend.sea_gradient * sea.x,
        sea.w * blend.sea * blend.sea,
    );
}

// The horizontal displacement of every cascade at grid point `at`, finest level, each scaled as a coast leaves it.
fn horizontal(at: vec2<f32>, scales: vec4<f32>) -> vec2<f32> {
    var sum = vec2<f32>(0.0);
    for (var i = 0; i < i32(waves.info.y); i++) {
        let copies = scatter(at, i);
        sum += mix_zero(copies_level(field_displacement, copies, i, 0.0), copies).xz * scales[i];
    }
    return sum;
}

// Surface height at a world position as a mesh whose vertices stand `spacing` metres apart there draws it: the grid
// point `displacement` moves onto `xz` (the same fixed-point steps as `height_at`), and its height, each cascade and
// combat's sea read and faded as the vertex stage reads them. A hull's sea is the water drawn against her, which far
// from the mesh's centre carries none of the waves `height_at` keeps (no TypeScript source: the web game's mesh was
// centred on the eye alone, and its hulls' sea was read within a few kilometres of it).
fn drawn_height_at(xz: vec2<f32>, spacing: f32) -> f32 {
    var grid = xz;
    var moved = displacement(grid, spacing);
    for (var step = 0; step < INVERSION_STEPS; step++) {
        let target_point = xz - moved.xz;
        if step == 0 {
            grid = target_point;
        } else {
            grid += (target_point - grid) * INVERSION_DAMPING;
        }
        moved = displacement(grid, spacing);
    }
    return moved.y;
}

// Surface height at a world position, any stage (`heightAt`, without the wake): the choppy map inverted with six fixed-
// point steps (the grid point whose displaced position is `xz`), and near hulls the long waves given way to combat's
// sea, the blend and the coast's scales read once at `xz`.
fn height_at(xz: vec2<f32>) -> f32 {
    let shore = shore_at(xz);
    let hulls = hull_blend(hull_weight(xz), vec2<f32>(0.0));
    let lowpass = hulls.lowpass * shore.scales.x;
    var long = long_wave(field_displacement, xz, 0.0).xz * lowpass;
    var grid = xz;
    for (var step = 0; step < INVERSION_STEPS; step++) {
        let target_point = xz - (horizontal(grid, shore.scales) + long);
        if step == 0 {
            grid = target_point;
            long = long_wave(field_displacement, grid, 0.0).xz * lowpass;
        } else {
            grid += (target_point - grid) * INVERSION_DAMPING;
        }
    }
    var height = 0.0;
    for (var i = 0; i < i32(waves.info.y); i++) {
        let at = scatter(grid, i);
        height += mix_zero(copies_level(field_displacement, at, i, 0.0), at).y * shore.scales[i];
    }
    return height + long_wave(field_displacement, grid, 0.0).y * lowpass
        + hull_height(xz, 0.0, shore.scales.w) * hulls.sea;
}
