struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

override ENABLE_LEGACY_FOG: bool = false;

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

struct EntityLightingSettings {
    values: vec4<u32>,
    legacy_fog_color_depth: vec4<f32>,
    legacy_fog_params: vec4<f32>,
};
@group(1) @binding(2) var<uniform> entity_lighting: EntityLightingSettings;

struct InstanceIn {
    @location(0) origin: vec4<f32>,
    @location(1) left: vec4<f32>,
    @location(2) up: vec4<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world_position: vec3<f32>,
};

@vertex
fn vs_main(input: InstanceIn, @builtin(vertex_index) vertex_index: u32) -> VertexOut {
    // CPU RB_AddQuadStamp order: (v0,v1,v3), (v3,v1,v2). Cull is disabled
    // for this pipeline, so the duplicate reverse-winding CPU triangles are
    // unnecessary while preserving the exact visible quad.
    let corners = array<vec2<f32>, 6>(
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
    );
    let uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let world = input.origin.xyz + input.left.xyz * corner.x + input.up.xyz * corner.y;
    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    output.uv = uvs[vertex_index];
    output.color = input.color;
    output.world_position = world;
    return output;
}

fn apply_self_legacy_fog(rgb: vec3<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec3<f32> {
    if (!ENABLE_LEGACY_FOG) {
        return rgb;
    }
    let mode = entity_lighting.legacy_fog_params.x;
    if (mode < 0.5) {
        return rgb;
    }
    let authored_depth = max(entity_lighting.legacy_fog_color_depth.a, 0.001);
    let scale = entity_lighting.legacy_fog_params.y;
    var amount = 0.0;
    if (mode < 1.5) {
        let forward_distance = max(
            dot(world_position - camera.camera_pos_time.xyz, normalize(camera.camera_forward.xyz)),
            0.0
        );
        let scaled = forward_distance / authored_depth * scale;
        amount = 1.0 - exp(-5.5412635 * scaled * scaled);
    } else {
        let radial = distance(world_position, camera.camera_pos_time.xyz) / authored_depth;
        amount = 1.0 - exp(-radial * scale * 0.26);
    }
    let fog_color = select(entity_lighting.legacy_fog_color_depth.rgb, vec3<f32>(0.0), fog_to_black);
    return mix(rgb, fog_color, clamp(amount, 0.0, 1.0));
}

fn fogged(color: vec4<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec4<f32> {
    return vec4<f32>(apply_self_legacy_fog(color.rgb, world_position, fog_to_black), color.a);
}

@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    return fogged(textureSample(base_texture, base_sampler, input.uv) * input.color, input.world_position, false);
}

@fragment
fn fs_mask(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    if tex.a < 0.5 {
        discard;
    }
    return fogged(tex * input.color, input.world_position, false);
}

@fragment
fn fs_unlit(input: VertexOut) -> @location(0) vec4<f32> {
    return fogged(textureSample(base_texture, base_sampler, input.uv) * input.color, input.world_position, false);
}

@fragment
fn fs_unlit_zero_alpha(input: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(base_texture, base_sampler, input.uv) * input.color;
    // Exact-zero only: this is an A/B for guaranteed transparent source pixels,
    // not an alpha cutoff. Bilinear edge samples with any non-zero alpha survive.
    if color.a <= 0.0 {
        discard;
    }
    return fogged(color, input.world_position, false);
}

@fragment
fn fs_unlit_fog_black(input: VertexOut) -> @location(0) vec4<f32> {
    return fogged(textureSample(base_texture, base_sampler, input.uv) * input.color, input.world_position, true);
}
