// The main menu's HDR neon_signs.wgsl and the identical Jimenez bloom chain
// feed this *only* final presentation stage. No fake outline shader, no
// precipitation or frosted glass. Overlays the luminous emission on the scene.
@group(0) @binding(0) var emission: texture_2d<f32>;
@group(0) @binding(1) var halo: texture_2d<f32>;
@group(0) @binding(2) var linear_sampler: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Out {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0,-3.0), vec2<f32>(-1.0,1.0), vec2<f32>(3.0,1.0))[index];
    var out: Out;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * vec2<f32>(0.5,-0.5) + 0.5;
    return out;
}
@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let tube = textureSampleLevel(emission, linear_sampler, in.uv, 0.0).rgb;
    let bloom = textureSampleLevel(halo, linear_sampler, in.uv, 0.0).rgb;
    // Same HDR neon energy as Main, with a slightly restrained atmosphere.
    // Luminous contribution is not pre-multiplied by transparency: light is
    // an additive source. Alpha only attenuates the underlying scene near
    // the physical glass tube. This keeps the glass core bright on any map.
    let energy = max(tube * 0.78 + bloom * 0.20, vec3<f32>(0.0));
    let light = vec3<f32>(1.0) - exp(-energy * 1.2);
    let opacity = clamp(max(light.r, max(light.g, light.b)) * 0.61, 0.0, 0.92);
    return vec4<f32>(light, opacity);
}
