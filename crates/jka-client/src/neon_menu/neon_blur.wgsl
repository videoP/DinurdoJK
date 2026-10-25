// Downsample/upsample chain (Jimenez, "Next Generation Post Processing in Call
// of Duty: Advanced Warfare"): 13-tap downsample, 3x3 tent upsample that adds
// the current level. Used twice by the menu: neon halo, and the fogged glass.
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var current: texture_2d<f32>;
@group(0) @binding(2) var linear_sampler: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Out {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0))[index];
    var out: Out;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * vec2<f32>(0.5, -0.5) + 0.5;
    return out;
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source, linear_sampler, uv, 0.0).rgb;
}

@fragment
fn fs_down(in: Out) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(source));
    let uv = in.uv;
    let a = tap(uv + t * vec2<f32>(-2.0, -2.0));
    let b = tap(uv + t * vec2<f32>(0.0, -2.0));
    let c = tap(uv + t * vec2<f32>(2.0, -2.0));
    let d = tap(uv + t * vec2<f32>(-2.0, 0.0));
    let e = tap(uv);
    let f = tap(uv + t * vec2<f32>(2.0, 0.0));
    let g = tap(uv + t * vec2<f32>(-2.0, 2.0));
    let h = tap(uv + t * vec2<f32>(0.0, 2.0));
    let i = tap(uv + t * vec2<f32>(2.0, 2.0));
    let j = tap(uv + t * vec2<f32>(-1.0, -1.0));
    let k = tap(uv + t * vec2<f32>(1.0, -1.0));
    let l = tap(uv + t * vec2<f32>(-1.0, 1.0));
    let m = tap(uv + t * vec2<f32>(1.0, 1.0));
    var color = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
    return vec4<f32>(max(color, vec3<f32>(0.0)), 1.0);
}

@fragment
fn fs_up(in: Out) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(source));
    let uv = in.uv;
    var lower = tap(uv) * 4.0;
    lower += (tap(uv + vec2<f32>(t.x, 0.0)) + tap(uv - vec2<f32>(t.x, 0.0))
        + tap(uv + vec2<f32>(0.0, t.y)) + tap(uv - vec2<f32>(0.0, t.y))) * 2.0;
    lower += tap(uv + t) + tap(uv - t) + tap(uv + vec2<f32>(t.x, -t.y)) + tap(uv + vec2<f32>(-t.x, t.y));
    let here = textureSampleLevel(current, linear_sampler, uv, 0.0).rgb;
    return vec4<f32>(here + lower / 16.0, 1.0);
}
