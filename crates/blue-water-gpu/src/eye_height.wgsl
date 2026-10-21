// Height of the surface the ORIGINAL Blue Water mesh draws over one XZ point (metres, upstream
// frame: y = 0 at the water plane). The mesh draws a vertex at `grid + displacement(grid)`, so the
// surface over a point is found by inverting that horizontal displacement. `displacement` is the
// preserved upstream function itself, including its stochastic tiling of the two long cascades.
#import naval_ocean::waves::{displacement, shore_at}

struct EyeParams {
    // xz metres, x/z of the grid spacing the mesh uses at the eye, unused
    eye: vec4<f32>,
    swell: array<vec4<f32>, 2>,
    // `r_oceanType 5` (godot_water.wgsl): x 1 on, y wave scale, z seconds.
    godot: vec4<f32>,
}
@group(0) @binding(0) var<uniform> probe: EyeParams;
@group(0) @binding(1) var<storage, read_write> result: array<vec4<f32>, 2>;

fn jka_native_raw_offset(grid: vec2<f32>, spacing: f32) -> vec3<f32> {
    var base = vec3<f32>(0.0);
    if probe.swell[1].z > 0.0 {
        base = displacement(grid, spacing) * probe.swell[1].z;
    }
    let amplitude = probe.swell[0].w * shore_at(grid).scales.w;
    if amplitude <= 0.0 { return base; }
    let phase = dot(grid, probe.swell[0].xy) * probe.swell[0].z
        - probe.swell[1].x * probe.swell[1].w;
    let horizontal = amplitude * probe.swell[1].y * cos(phase);
    return base + vec3<f32>(probe.swell[0].x * horizontal,
        amplitude * sin(phase), probe.swell[0].y * horizontal);
}

fn jka_native_offset(grid: vec2<f32>, spacing: f32) -> vec3<f32> {
    if probe.godot.x > 0.5 {
        return jka_godot_displacement(grid, spacing, probe.godot.y, probe.godot.z);
    }
    let raw = jka_native_raw_offset(grid, spacing);
    let scale = jka_crest_scale(probe.swell[0], probe.swell[1], probe.eye.w);
    return raw * vec3<f32>(scale, 1.0, scale);
}

@compute @workgroup_size(1)
fn main() {
    let target_xz = probe.eye.xy;
    let spacing = probe.eye.z;
    var grid = target_xz;
    var d = jka_native_offset(grid, spacing);
    for (var i = 0u; i < 6u; i = i + 1u) {
        grid = target_xz - d.xz;
        d = jka_native_offset(grid, spacing);
    }
    let residual = length(grid + d.xz - target_xz);
    result[1] = vec4<f32>(d.y, residual, 1.0, 0.0);
}
