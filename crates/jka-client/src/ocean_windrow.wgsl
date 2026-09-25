// Persistent severe-wind surface foam transport.
// Existing Jacobian whitecaps are the source; this pass only transports and
// concentrates that foam into long wind-aligned convergence streaks.

const WINDROW_SIZE: u32 = 256u;
const WINDROW_CELLS: u32 = WINDROW_SIZE * WINDROW_SIZE;

struct WindrowParams {
    map_scales: array<vec4<f32>, 3>,
    wind_time: vec4<f32>, // wind XZ m/s, elapsed seconds, frame delta
    params: vec4<f32>,    // domain metres, source strength, lifetime seconds, previous layer
};

@group(0) @binding(0) var<storage, read_write> foam_field: array<f32>;
@group(0) @binding(1) var breaker_normal: texture_2d_array<f32>;
@group(0) @binding(2) var repeat_sampler: sampler;
@group(0) @binding(3) var<uniform> state: WindrowParams;

fn smooth01(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = clamp((x-edge0)/max(edge1-edge0, 1e-5), 0.0, 1.0);
    return t*t*(3.0-2.0*t);
}

fn wrap_cell(v: i32) -> u32 {
    let n = i32(WINDROW_SIZE);
    return u32((v % n + n) % n);
}

fn field_at(layer: u32, x: i32, y: i32) -> f32 {
    let ix = wrap_cell(x);
    let iy = wrap_cell(y);
    return foam_field[layer * WINDROW_CELLS + iy * WINDROW_SIZE + ix];
}

fn sample_previous(uv_in: vec2<f32>, layer: u32) -> f32 {
    let uv = fract(uv_in);
    let p = uv * f32(WINDROW_SIZE) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(p));
    let f = fract(p);
    let a = field_at(layer, base.x, base.y);
    let b = field_at(layer, base.x + 1, base.y);
    let c = field_at(layer, base.x, base.y + 1);
    let d = field_at(layer, base.x + 1, base.y + 1);
    return mix(mix(a,b,f.x), mix(c,d,f.x), f.y);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= WINDROW_SIZE || gid.y >= WINDROW_SIZE) { return; }

    let domain = max(state.params.x, 1.0);
    let uv = (vec2<f32>(gid.xy) + vec2<f32>(0.5)) / f32(WINDROW_SIZE);
    let world_m = uv * domain;
    let dt = clamp(state.wind_time.w, 0.0, 0.1);
    let wind = state.wind_time.xy;
    let speed = length(wind);
    let active = smooth01(8.0, 14.0, speed);
    var wind_dir = vec2<f32>(1.0, 0.0);
    if (speed > 1e-4) { wind_dir = wind / speed; }
    let cross_dir = vec2<f32>(-wind_dir.y, wind_dir.x);

    // Broad, slowly warped convergence cells. The scalar foam field remains the
    // visible source of truth; these functions only define transport velocity.
    let along = dot(world_m, wind_dir);
    let across = dot(world_m, cross_dir);
    let spacing = mix(42.0, 24.0, smooth01(10.0, 24.0, speed));
    let phase = 6.28318530718 * across / spacing
        + 0.62 * sin(along * 0.035 + state.wind_time.z * 0.020)
        + 0.24 * sin(along * 0.083 - state.wind_time.z * 0.014);
    let convergence_speed = min(0.80, speed * 0.035) * active;
    let cross_velocity = -sin(phase) * convergence_speed;
    let drift = wind * 0.030;
    let velocity = drift + cross_dir * cross_velocity;

    let previous_layer = u32(clamp(state.params.w, 0.0, 1.0) + 0.5);
    let next_layer = 1u - previous_layer;
    let previous_uv = (world_m - velocity * dt) / domain;
    var foam = sample_previous(previous_uv, previous_layer);

    // Semi-Lagrangian scalar advection does not conserve density by itself. A
    // small compression term makes actual convergence bands collect foam instead
    // of merely translating the pattern.
    let compression = max(cos(phase), 0.0) * active;
    foam *= 1.0 + compression * dt * 0.85;

    // Existing accumulated FFT/Jacobian foam is the only source. A fairly high
    // threshold keeps isolated ordinary whitecaps distinct from the long-lived
    // floating material that severe wind organizes into windrows.
    var breaker = 0.0;
    for (var i = 0u; i < 3u; i = i + 1u) {
        let source_uv = world_m * vec2<f32>(1.0, -1.0) * state.map_scales[i].xy;
        breaker += textureSampleLevel(breaker_normal, repeat_sampler, source_uv, i32(i), 0.0).w;
    }
    let source = smooth01(0.55, 1.35, breaker) * active * state.params.y;
    foam += source * dt * 0.18;

    // Surface foam persists much longer than crest spray but eventually clears.
    let lifetime = max(state.params.z, 0.1);
    foam *= exp(-dt / lifetime);
    foam = clamp(foam, 0.0, 1.0);

    let index = next_layer * WINDROW_CELLS + gid.y * WINDROW_SIZE + gid.x;
    foam_field[index] = foam;
}
