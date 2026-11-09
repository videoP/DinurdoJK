// Baked Blue Water dome. Exact upstream dome_sky + celestial math, evaluated
// once in six world-space directions rather than every screen pixel per frame.
// The live dome's camera-dependent aerial overlay stays in its own world pass.
#import naval_sky::atmosphere::{dome_sky, transmittance_to_space}
#import naval_sky::celestial::{celestial_radiance}

struct CubeVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) clip_xy: vec2<f32>,
    @location(1) @interpolate(flat) face: u32,
}
@vertex fn cube_vertex(@builtin(vertex_index) vertex: u32,
                       @builtin(instance_index) face: u32) -> CubeVertex {
    let clip = vec2<f32>(select(-1.0, 3.0, vertex == 1u),
                         select(-1.0, 3.0, vertex == 2u));
    var out: CubeVertex;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.clip_xy = clip;
    out.face = face;
    return out;
}
// WebGPU cube face axes, matching texture_cube sampling in the display pass.
fn cube_direction(face: u32, uv: vec2<f32>) -> vec3<f32> {
    let x = uv.x;
    let y = uv.y;
    switch (face) {
        case 0u: { return normalize(vec3<f32>( 1.0,  y, -x)); } // +X
        case 1u: { return normalize(vec3<f32>(-1.0,  y,  x)); } // -X
        case 2u: { return normalize(vec3<f32>( x,  1.0, -y)); } // +Y
        case 3u: { return normalize(vec3<f32>( x, -1.0,  y)); } // -Y
        case 4u: { return normalize(vec3<f32>( x,  y,  1.0)); } // +Z
        default: { return normalize(vec3<f32>(-x, y, -1.0)); } // -Z
    }
}
@fragment fn cube_fragment(input: CubeVertex) -> @location(0) vec4<f32> {
    let direction = cube_direction(input.face, input.clip_xy);
    let radiance = dome_sky(direction, false, 0.0)
        + transmittance_to_space(direction, false) * celestial_radiance(direction);
    return vec4<f32>(radiance, 1.0);
}
