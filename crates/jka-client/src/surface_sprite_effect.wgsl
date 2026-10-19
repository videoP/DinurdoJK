struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) alpha_cutoff: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) alpha_cutoff: f32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = camera.view_proj * vec4<f32>(input.position, 1.0);
    output.uv = input.uv;
    output.color = input.color;
    output.alpha_cutoff = input.alpha_cutoff;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(sprite_texture, sprite_sampler, input.uv);
    // AlphaFunc GE192/GE128/GT0 tests sampled alpha multiplied by vertex fade.
    // Discard *before* depth write: opaque One/Zero stages must not paint a quad.
    if input.alpha_cutoff > 0.0 && texel.a * input.color.a < input.alpha_cutoff {
        discard;
    }
    if input.alpha_cutoff < 0.0 && texel.a * input.color.a >= -input.alpha_cutoff {
        discard;
    }
    return texel * input.color;
}


@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
