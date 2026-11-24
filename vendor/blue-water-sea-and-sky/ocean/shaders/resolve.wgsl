// Ports `resolve` and `spreadFoam` of the web game's `ocean/waves/field.ts`: the last column pass of the inverse transform for
// every cascade layer, which writes the spatial fields and persists crest foam.
//
// Per layer: displacement (Dx, Dy, Dz, streaks), derivatives (∂y/∂x, ∂y/∂z, ∂Dx/∂x, ∂Dz/∂z) and extras (∂Dx/∂z, foam,
// (∂y/∂x)² + (∂y/∂z)², bubbles). The squared slope is mip-filtered with the slope, so E[s²] − E[s]² is the slope
// variance inside any pixel's filter footprint (LEAN mapping). Crests break where they are compressed along the wind
// and steep on their forward face (the web game's `ocean/waves/whitecaps.ts`); foam and bubbles decay, spread downwind and
// take fresh injection, reading last update's extras.
//
// Native only: in a gale the foam the whitecaps leave on the water drifts downwind with the surface's current and
// gathers into long streaks along the wind (Beaufort 8 to 10's "well-marked streaks of foam"). On the cascades that
// carry them (`c.z` > 0) the displacement's w holds that old foam: last update's, carried by the drift and drawn toward
// lines along the wind (Langmuir circulation's convergences, meandering and periodic on the tile), decaying over tens of
// seconds and fed by the whitecaps' foam.

struct CascadeParams {
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
    d: vec4<f32>,
}

struct FieldParams {
    time: vec4<f32>,
    wind: vec4<f32>,
    cascades: array<CascadeParams, 3>,
    // DinurdoJK: xyz = 1 when cascade 0/1/2 carries visible waves (see fft.wgsl).
    cascade_on: vec4<f32>,
}

struct PassParams {
    radix: u32,
    span: u32,
    horizontal: u32,
    evolve: u32,
}

struct Pair {
    a: vec4<f32>,
    b: vec4<f32>,
}

@group(0) @binding(0) var<uniform> field: FieldParams;
@group(0) @binding(1) var<uniform> pass_params: PassParams;
@group(0) @binding(2) var<storage, read> source: array<Pair>;
@group(0) @binding(3) var previous_extras: texture_2d_array<f32>;
@group(0) @binding(4) var displacement: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(5) var derivatives: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(6) var extras: texture_storage_2d_array<rgba16float, write>;
// Last update's displacement, whose w holds the streaks.
@group(0) @binding(7) var previous_displacement: texture_2d_array<f32>;

const TAU: f32 = 6.283185307179586;
// Injection ramps from nothing to fresh foam over BREAKING_RAMP / z of the indicator, centred on the threshold z.
const BREAKING_RAMP: f32 = 3.0;
// The most old foam a texel of streaks holds.
const STREAKS_MOST: f32 = 1.5;

fn rotate(value: vec4<f32>, c: f32, s: f32) -> vec4<f32> {
    return value * c + vec4<f32>(-value.y, value.x, -value.w, value.z) * s;
}

// Last update's foam and bubbles at a texel of this layer, wrapping at the tile's edge.
fn previous_foam(pixel: vec2<i32>, layer: i32, n: i32) -> vec2<f32> {
    let at = (pixel + vec2<i32>(n)) % vec2<i32>(n);
    return textureLoad(previous_extras, at, layer, 0).yw;
}

// Last update's streaks at texel position `at` (texel units, wrapped on the tile), bilinear.
fn previous_streaks(at: vec2<f32>, layer: i32, n: i32) -> f32 {
    let base = floor(at);
    let f = at - base;
    let i = vec2<i32>(base);
    let s00 = textureLoad(previous_displacement, ((i % n) + n) % n, layer, 0).w;
    let s10 = textureLoad(previous_displacement, (((i + vec2<i32>(1, 0)) % n) + n) % n, layer, 0).w;
    let s01 = textureLoad(previous_displacement, (((i + vec2<i32>(0, 1)) % n) + n) % n, layer, 0).w;
    let s11 = textureLoad(previous_displacement, (((i + vec2<i32>(1, 1)) % n) + n) % n, layer, 0).w;
    return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y);
}

// This update's streaks at texel `pixel` of a layer whose whitecap foam there is `foam`: last update's, carried back
// along the drift and the gathering toward the lines, decayed, and fed.
fn streaks(pixel: vec2<i32>, layer: i32, n: i32, cascade: CascadeParams, foam: f32, elapsed: f32) -> f32 {
    let p = vec2<f32>(pixel);
    // The lines: a lattice wave across the wind, its phase wandering along it.
    let k = cascade.d.xy * (TAU / f32(n));
    let across = cascade.d.xy / max(length(cascade.d.xy), 1e-3);
    let along = round(vec2<f32>(-across.y, across.x)) * (TAU / f32(n));
    let wander = dot(along, p) + field.time.w;
    let phase = dot(k, p) + cascade.d.w * sin(wander);
    let gradient = k + along * (cascade.d.w * cos(wander));
    let slope = length(gradient);
    // A second, wider set of cells a few degrees off the first, at half its strength: lines merge, fork and fade.
    let turned = round(vec2<f32>(cascade.d.x * 0.99 - cascade.d.y * 0.14, cascade.d.x * 0.14 + cascade.d.y * 0.99) * 0.62);
    let k2 = turned * (TAU / f32(n));
    let phase2 = dot(k2, p) - 0.5 * cascade.d.w * sin(wander * 2.0 + 1.3);
    let slope2 = max(length(k2), 1e-6);
    let gather = gradient * (-sin(phase) * cascade.d.z / max(slope, 1e-6)) + k2 * (-sin(phase2) * 0.5 * cascade.d.z / slope2);
    let carried = previous_streaks(p - (cascade.c.xy + gather) * elapsed, layer, n);
    // Foam is a density the current carries: where it converges (the lines) it thickens, where it parts it thins.
    let squeeze = cos(phase) * cascade.d.z * slope + cos(phase2) * 0.5 * cascade.d.z * slope2;
    return min(carried * exp((squeeze - 1.0 / cascade.c.z) * elapsed) + foam * cascade.c.w * elapsed, STREAKS_MOST);
}

@compute @workgroup_size(8, 8, 1)
fn resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = i32(field.wind.z);
    let width = n * i32(field.wind.w);
    let pixel = vec2<i32>(id.xy);
    let layer = i32(id.z);
    if pixel.x >= n || pixel.y >= n || layer >= i32(field.wind.w) {
        return;
    }
    // DinurdoJK: an inactive cascade (its FFT skipped) is flat: zeros keep both output halves and every reader
    // consistent, and a reactivated cascade starts without stale foam.
    if field.cascade_on[layer] < 0.5 {
        textureStore(displacement, pixel, layer, vec4<f32>(0.0));
        textureStore(derivatives, pixel, layer, vec4<f32>(0.0));
        textureStore(extras, pixel, layer, vec4<f32>(0.0));
        return;
    }
    let radix = i32(pass_params.radix);
    let span = i32(pass_params.span);
    let j = pixel.y;
    let block = radix * span;
    let first = (j / block) * span + j % span;
    let angle = f32(j % block) * (TAU / f32(block));
    var ab = vec4<f32>(0.0);
    var cd = vec4<f32>(0.0);
    for (var r = 0; r < radix; r++) {
        let value = source[(first + r * (n / radix)) * width + layer * n + pixel.x];
        let turn = angle * f32(r);
        let c = cos(turn);
        let s = sin(turn);
        ab += rotate(value.a, c, s);
        cd += rotate(value.b, c, s);
    }

    // Crests break where they are compressed along the wind and steep on their forward face.
    let cascade = field.cascades[layer];
    let wx = field.wind.x;
    let wz = field.wind.y;
    let compression = -(cd.z * wx * wx + ab.w * wx * wz * 2.0 + cd.w * wz * wz);
    let face = -(cd.x * wx + cd.y * wz);
    let indicator = compression * cascade.b.x + face * cascade.b.y;
    let threshold = cascade.a.z;
    let ramp = (BREAKING_RAMP / 2.0) / max(threshold, 1.0);
    let injection = smoothstep(threshold - ramp, threshold + ramp, indicator);

    // Last update's foam and bubbles after one explicit diffusion step, more along the wind than across it.
    let centre = previous_foam(pixel, layer, n);
    let along_x = previous_foam(pixel + vec2<i32>(-1, 0), layer, n) + previous_foam(pixel + vec2<i32>(1, 0), layer, n) - centre * 2.0;
    let along_z = previous_foam(pixel + vec2<i32>(0, -1), layer, n) + previous_foam(pixel + vec2<i32>(0, 1), layer, n) - centre * 2.0;
    let spread = centre + along_x * cascade.b.z + along_z * cascade.b.w;
    let elapsed = field.time.z;
    var foam = centre.x;
    var bubbles = centre.y;
    if elapsed > 0.0 {
        foam = max(spread.x * exp(-elapsed / cascade.a.y), injection);
        bubbles = max(spread.y * exp(-elapsed / cascade.a.w), injection);
    }

    var streak = 0.0;
    if cascade.c.z > 0.0 {
        streak = streaks(pixel, layer, n, cascade, foam, elapsed);
    }
    textureStore(displacement, pixel, layer, vec4<f32>(ab.xyz, streak));
    textureStore(derivatives, pixel, layer, cd);
    textureStore(extras, pixel, layer, vec4<f32>(ab.w, foam, cd.x * cd.x + cd.y * cd.y, bubbles));
}
