// Native addressing for shallow.wgsl: one dense rectangle, as in UE 5.8.1
// Grid2D_SW_River. Water/velocity attributes are R32f array layers with
// hardware bilinear sampling (Niagara SF_Bilinear, AM_Clamp).
@group(0) @binding(1) var water: texture_2d_array<f32>; // height, depth, vx, vy layers
@group(0) @binding(3) var output_water: texture_storage_2d_array<r32float, write>;
@group(0) @binding(11) var grid_sampler: sampler;

fn invocation_cell(id: vec3<u32>) -> vec2<i32> { return vec2<i32>(id.xy); }
fn invocation_active(id: vec3<u32>, at: vec2<i32>) -> bool { return in_bounds(at); }

// Integer Niagara grid loads outside the resource return zero. Sampling and
// extrapolation stages have their own policies and must not reuse this as a
// substitute for filtered texture sampling.
fn in_bounds(at: vec2<i32>) -> bool {
    return all(at >= vec2<i32>(0)) && all(at < vec2<i32>(params.dimensions.xy));
}
fn index(at: vec2<i32>) -> u32 { return u32(at.y) * params.dimensions.x + u32(at.x); }
fn read_water(at: vec2<i32>) -> vec4<f32> {
    if !in_bounds(at) { return vec4<f32>(0.0); }
    return vec4<f32>(textureLoad(water, at, 0, 0).x, textureLoad(water, at, 1, 0).x,
                     textureLoad(water, at, 2, 0).x, textureLoad(water, at, 3, 0).x);
}
fn write_water(at: vec2<i32>, value: vec4<f32>) {
    textureStore(output_water, at, 0, vec4<f32>(value.x));
    textureStore(output_water, at, 1, vec4<f32>(value.y));
    textureStore(output_water, at, 2, vec4<f32>(value.z));
    textureStore(output_water, at, 3, vec4<f32>(value.w));
}
fn read_geometry(at: vec2<i32>) -> vec4<f32> {
    if !in_bounds(at) { return vec4<f32>(0.0); }
    return geometry[index(at)];
}

fn clamp_cell(at: vec2<i32>) -> vec2<i32> {
    return clamp(at, vec2<i32>(0), vec2<i32>(params.dimensions.xy) - 1);
}

fn bilinear(unit: vec2<f32>, channel: u32) -> f32 {
    // Niagara's SF_Bilinear / AM_Clamp SampleLevel uses the original UV, not
    // a UV reconstructed from GridPos. Let the GPU perform texture filtering.
    return textureSampleLevel(water, grid_sampler, unit, i32(channel), 0.0).x;
}
