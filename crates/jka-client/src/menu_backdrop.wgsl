struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 2.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(2.0, 0.0),
    );
    var out: VertexOut;
    out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    out.uv = uvs[vertex_index];
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(source_texture));
    let texel = 2.0 / max(dims, vec2<f32>(1.0));
    let uv = input.uv;

    // A compact 3x3 Gaussian kernel. This pass is lazy and only exists after a
    // menu page requests the blurred/desaturated live backdrop. Gameplay and
    // the VIDEO setup tab pay no fullscreen-pass cost.
    var color = textureSample(source_texture, source_sampler, uv).rgb * 4.0;
    color += textureSample(source_texture, source_sampler, uv + vec2<f32>( texel.x, 0.0)).rgb * 2.0;
    color += textureSample(source_texture, source_sampler, uv + vec2<f32>(-texel.x, 0.0)).rgb * 2.0;
    color += textureSample(source_texture, source_sampler, uv + vec2<f32>(0.0,  texel.y)).rgb * 2.0;
    color += textureSample(source_texture, source_sampler, uv + vec2<f32>(0.0, -texel.y)).rgb * 2.0;
    color += textureSample(source_texture, source_sampler, uv + vec2<f32>( texel.x,  texel.y)).rgb;
    color += textureSample(source_texture, source_sampler, uv + vec2<f32>(-texel.x,  texel.y)).rgb;
    color += textureSample(source_texture, source_sampler, uv + vec2<f32>( texel.x, -texel.y)).rgb;
    color += textureSample(source_texture, source_sampler, uv + vec2<f32>(-texel.x, -texel.y)).rgb;
    color *= 1.0 / 16.0;

    let luma = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    color = mix(color, vec3<f32>(luma), 0.30);
    return vec4<f32>(color, 1.0);
}
