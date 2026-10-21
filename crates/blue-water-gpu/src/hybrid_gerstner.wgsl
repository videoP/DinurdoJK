// JKA-only Hybrid directional Gerstner, independent of Blue Water's FFT update.
// A 48-byte uniform; mode 0 + 1.0 FFT mix + 0 analytic amplitude preserves native Blue Water.
#import naval_ocean::waves as jka_coastal_waves
struct JkaHybridSwell {
    direction_wave: vec4<f32>, // xy unit travel dir, z radians/m, w Gerstner amplitude (m)
    motion_mix: vec4<f32>,     // x radians/s, y horizontal ratio, z FFT weight, w seconds
    breaker: vec4<f32>,        // x A/B mode: 0 original, 1 limit, 2 spilling crests
    // `r_oceanType 5` (godot_water.wgsl): x 1 on, y wave scale, z seconds, w foam bits (1 near, 2 far, 4 distant).
    godot: vec4<f32>,
    // The persistent foam maps' capture centres (grid xz, m): xy near, zw far (each where its map was last drawn).
    godot_foam: vec4<f32>,
    // rgb: the water colour (linear; upstream's `color`, `r_gerstnerWaterColor`).
    godot_color: vec4<f32>,
    // rgb: the colour crests blend toward (linear; upstream's fixed 0, 0.08, 0.02, `r_gerstnerCrestColor`).
    godot_crest: vec4<f32>,
}
@group(1) @binding(17) var<uniform> jka_hybrid: JkaHybridSwell;
// r: sea_foam.png, g: the seamless cellular noise (ocean and erosion noise), b: the smooth simplex noise (i_noise).
@group(1) @binding(18) var jka_godot_patterns: texture_2d<f32>;
@group(1) @binding(19) var jka_godot_repeat: sampler;
// The persistent foam maps (r: Jacobian foam, g: intersection foam), each centred on `godot_foam.xy`.
@group(1) @binding(20) var jka_godot_foam_near: texture_2d<f32>;
@group(1) @binding(21) var jka_godot_foam_far: texture_2d<f32>;

fn jka_godot_noise(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(jka_godot_patterns, jka_godot_repeat, uv, 0.0).g;
}

fn jka_hybrid_raw_displacement(grid: vec2<f32>, spacing: f32) -> vec3<f32> {
    if jka_hybrid.godot.x > 0.5 {
        return jka_godot_displacement(grid, spacing, jka_hybrid.godot.y, jka_hybrid.godot.z);
    }
    var base = vec3<f32>(0.0);
    if jka_hybrid.motion_mix.z > 0.0 {
        base = displacement(grid, spacing) * jka_hybrid.motion_mix.z;
    }
    let a = jka_hybrid.direction_wave.w * jka_coastal_waves::shore_at(grid).scales.w;
    if a <= 0.0 { return base; }
    let phase = dot(grid, jka_hybrid.direction_wave.xy) * jka_hybrid.direction_wave.z
        - jka_hybrid.motion_mix.x * jka_hybrid.motion_mix.w;
    let horizontal = a * jka_hybrid.motion_mix.y * cos(phase);
    return base + vec3<f32>(jka_hybrid.direction_wave.x * horizontal,
        a * sin(phase), jka_hybrid.direction_wave.y * horizontal);
}

fn jka_hybrid_crest_scale() -> f32 {
    return jka_crest_scale(jka_hybrid.direction_wave, jka_hybrid.motion_mix, jka_hybrid.breaker.x);
}

fn jka_hybrid_displacement(grid: vec2<f32>, spacing: f32) -> vec3<f32> {
    let raw = jka_hybrid_raw_displacement(grid, spacing);
    let scale = jka_hybrid_crest_scale();
    return raw * vec3<f32>(scale, 1.0, scale);
}

// Tile LOD transitions must land on the same corrected parent triangle.
// Keep the original parent's evaluation byte-for-byte in the off path.
fn jka_hybrid_parent_displacement(grid: vec2<f32>, spacing: f32) -> vec3<f32> {
    if jka_hybrid.breaker.x > 0.5 || jka_hybrid.godot.x > 0.5 { return jka_hybrid_displacement(grid, spacing); }
    return displacement(grid, spacing);
}

fn jka_hybrid_strain(grid: vec2<f32>) -> vec3<f32> {
    let a = jka_hybrid.direction_wave.w * jka_coastal_waves::shore_at(grid).scales.w;
    let direction = jka_hybrid.direction_wave.xy;
    let phase = dot(grid, direction) * jka_hybrid.direction_wave.z
        - jka_hybrid.motion_mix.x * jka_hybrid.motion_mix.w;
    let rate = -a * jka_hybrid.motion_mix.y * jka_hybrid.direction_wave.z * sin(phase);
    return vec3<f32>(direction.x * direction.x, direction.y * direction.y, direction.x * direction.y) * rate;
}

fn jka_hybrid_slope(grid: vec2<f32>) -> vec2<f32> {
    let a = jka_hybrid.direction_wave.w * jka_coastal_waves::shore_at(grid).scales.w;
    if a <= 0.0 { return vec2<f32>(0.0); }
    let phase = dot(grid, jka_hybrid.direction_wave.xy) * jka_hybrid.direction_wave.z
        - jka_hybrid.motion_mix.x * jka_hybrid.motion_mix.w;
    return jka_hybrid.direction_wave.xy * (a * jka_hybrid.direction_wave.z * cos(phase));
}
