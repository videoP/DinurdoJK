struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(input.position, 1.0);
    output.uv = input.uv;
    output.normal = normalize(input.normal);
    output.color = input.color;
    return output;
}

fn lit_color(normal: vec3<f32>, tex: vec4<f32>) -> vec4<f32> {
    // Lightweight JKA-style vertex-normal lighting: ambient plus directed light.
    // The entity path stays independent of BSP lightmaps for this first cgame
    // presentation milestone; ambient-cube/entity lighting can replace this
    // without changing snapshot/Ghoul2 ownership.
    let light_dir = normalize(vec3<f32>(0.35, 0.75, 0.55));
    let diffuse = max(dot(normalize(normal), light_dir), 0.0);
    let lighting = 0.32 + 0.68 * diffuse;
    return vec4<f32>(tex.rgb * lighting, tex.a);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return lit_color(input.normal, tex) * input.color;
}

@fragment
fn fs_mask(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    if tex.a < 0.5 {
        discard;
    }
    return lit_color(input.normal, tex) * input.color;
}

@fragment
fn fs_unlit(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return tex * input.color;
}
