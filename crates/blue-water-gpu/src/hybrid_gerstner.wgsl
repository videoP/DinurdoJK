// JKA-only Hybrid directional Gerstner, independent of Blue Water's FFT update.
// One 32-byte uniform; 1.0 FFT mix + 0 analytic amplitude reproduces native Blue Water.
#import naval_ocean::waves as jka_coastal_waves
struct JkaHybridSwell {
    direction_wave: vec4<f32>, // xy unit travel dir, z radians/m, w Gerstner amplitude (m)
    motion_mix: vec4<f32>,     // x radians/s, y horizontal ratio, z FFT weight, w seconds
}
@group(1) @binding(17) var<uniform> jka_hybrid: JkaHybridSwell;

fn jka_hybrid_displacement(grid: vec2<f32>, spacing: f32) -> vec3<f32> {
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

fn jka_hybrid_slope(grid: vec2<f32>) -> vec2<f32> {
    let a = jka_hybrid.direction_wave.w * jka_coastal_waves::shore_at(grid).scales.w;
    if a <= 0.0 { return vec2<f32>(0.0); }
    let phase = dot(grid, jka_hybrid.direction_wave.xy) * jka_hybrid.direction_wave.z
        - jka_hybrid.motion_mix.x * jka_hybrid.motion_mix.w;
    return jka_hybrid.direction_wave.xy * (a * jka_hybrid.direction_wave.z * cos(phase));
}
