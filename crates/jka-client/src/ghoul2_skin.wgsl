struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

// Pipeline-specialized: false compiles the self-applied Legacy fog out entirely.
// Rebuilt only when FogSystem::legacy_self_fog toggles, never checked per frame.
override ENABLE_LEGACY_FOG: bool = false;

struct BoneMatrix {
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
};

struct SkinDraw {
    axis0: vec4<f32>,
    axis1: vec4<f32>,
    axis2: vec4<f32>,
    origin: vec4<f32>,
    color: vec4<f32>,
    params: vec4<u32>,
    classic_ambient: vec4<f32>,
    classic_directed: vec4<f32>,
    classic_direction: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

struct EntityLightingSettings {
    values: vec4<u32>,
    // Self-applied Legacy fog: linear RGB, depthForOpaque.
    legacy_fog_color_depth: vec4<f32>,
    // x: 0 off, 1 authored global EXP2, 2 manual fog; y: strength scale.
    legacy_fog_params: vec4<f32>,
};
@group(1) @binding(2) var<uniform> entity_lighting: EntityLightingSettings;
@group(2) @binding(0) var<storage, read> bones: array<BoneMatrix>;
@group(2) @binding(1) var<storage, read> skin_draws: array<SkinDraw>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bone_indices: vec4<u32>,
    @location(4) weights: vec4<f32>,
    @location(5) weight_count: u32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world_position: vec3<f32>,
};

fn transform_point(bone: BoneMatrix, point: vec3<f32>) -> vec3<f32> {
    let p = vec4<f32>(point, 1.0);
    return vec3<f32>(dot(bone.row0, p), dot(bone.row1, p), dot(bone.row2, p));
}

fn transform_vector(bone: BoneMatrix, vector: vec3<f32>) -> vec3<f32> {
    let v = vec4<f32>(vector, 0.0);
    return vec3<f32>(dot(bone.row0, v), dot(bone.row1, v), dot(bone.row2, v));
}

fn skin_position(
    position: vec3<f32>,
    bone_indices: vec4<u32>,
    weights: vec4<f32>,
    weight_count: u32,
    bone_base: u32,
) -> vec3<f32> {
    let p0 = transform_point(bones[bone_base + bone_indices.x], position);
    if weight_count <= 1u {
        return p0;
    }

    let p1 = transform_point(bones[bone_base + bone_indices.y], position);
    if weight_count == 2u {
        // Match OpenJK/RB_SurfaceGhoul's optimized two-weight form exactly.
        return weights.x * (p0 - p1) + p1;
    }

    let p2 = transform_point(bones[bone_base + bone_indices.z], position);
    if weight_count == 3u {
        let residual = 1.0 - weights.x - weights.y;
        return weights.x * p0 + weights.y * p1 + residual * p2;
    }

    let p3 = transform_point(bones[bone_base + bone_indices.w], position);
    let residual = 1.0 - weights.x - weights.y - weights.z;
    return weights.x * p0
        + weights.y * p1
        + weights.z * p2
        + residual * p3;
}

fn model_to_world(draw: SkinDraw, model_position: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        draw.origin.x + draw.axis0.x * model_position.x + draw.axis1.x * model_position.y + draw.axis2.x * model_position.z,
        draw.origin.y + draw.axis0.y * model_position.x + draw.axis1.y * model_position.y + draw.axis2.y * model_position.z,
        draw.origin.z + draw.axis0.z * model_position.x + draw.axis1.z * model_position.y + draw.axis2.z * model_position.z,
    );
}

// Same JKA -> renderer conversion as scene::render_position([x,y,z]).
fn jka_to_render(world: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(world.x, world.z, -world.y);
}

@vertex
fn vs_main(input: VertexIn, @builtin(instance_index) draw_index: u32) -> VertexOut {
    let draw = skin_draws[draw_index];
    let model_position = skin_position(
        input.position,
        input.bone_indices,
        input.weights,
        input.weight_count,
        draw.params.x,
    );

    // OpenJK's fast Ghoul2 path transforms the normal only by weight-0's bone.
    let model_normal = transform_vector(
        bones[draw.params.x + input.bone_indices.x],
        input.normal,
    );

    let world = model_to_world(draw, model_position);
    let world_normal = vec3<f32>(
        draw.axis0.x * model_normal.x + draw.axis1.x * model_normal.y + draw.axis2.x * model_normal.z,
        draw.axis0.y * model_normal.x + draw.axis1.y * model_normal.y + draw.axis2.y * model_normal.z,
        draw.axis0.z * model_normal.x + draw.axis1.z * model_normal.y + draw.axis2.z * model_normal.z,
    );

    let render_position = jka_to_render(world);
    let render_normal = normalize(vec3<f32>(world_normal.x, world_normal.z, -world_normal.y));

    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(render_position, 1.0);
    output.uv = input.uv;
    output.normal = render_normal;
    var draw_color = draw.color;
    if (draw.params.y != 0u) {
        var incoming = 0.0;
        let light_dir = draw.classic_direction.xyz;
        let light_len_sq = dot(light_dir, light_dir);
        if (light_len_sq > 1.0e-8) {
            incoming = max(dot(render_normal, light_dir * inverseSqrt(light_len_sq)), 0.0);
        }
        let lighting = clamp(
            (draw.classic_ambient.xyz + incoming * draw.classic_directed.xyz) / 255.0,
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        draw_color = vec4<f32>(draw.color.rgb * lighting, draw.color.a);
    }
    output.color = draw_color;
    output.world_position = render_position;
    return output;
}

fn lit_color(normal: vec3<f32>, tex: vec4<f32>) -> vec4<f32> {
    // BSP LIGHTGRID is already applied above from the per-draw OpenJK probe
    // sample. The previous directional placeholder remains only under
    // irradiance-volume mode until Ghoul2 is bound to the ambient-cube data.
    if (entity_lighting.values.x != 2u) {
        return tex;
    }
    let light_dir = normalize(vec3<f32>(0.35, 0.75, 0.55));
    let diffuse = max(dot(normalize(normal), light_dir), 0.0);
    let lighting = 0.32 + 0.68 * diffuse;
    return vec4<f32>(tex.rgb * lighting, tex.a);
}

// OpenJK r_drawfog 2 applies GL_EXP2 hardware fog to entities too: their fog
// num is the world's global fog, whose bounds cover the whole world. This is
// the same curve and colour decode as bsp.wgsl legacy_separate_fog, so a
// player and the floor at the same distance receive identical fog.
// Manual fog (no authored map fog) uses the same path in both Legacy modes;
// Legacy 1 authored global fog is instead applied in post from the prepass.
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
        // GL_EXP2 density ln(255)^0.5 / depthForOpaque over eye-forward depth.
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
    // OpenJK mGLFogColorOverride: GL_ONE GL_ONE and GL_DST_COLOR GL_ZERO stages
    // fog toward black so the blend equation does not add/multiply fog colour.
    let fog_color = select(entity_lighting.legacy_fog_color_depth.rgb, vec3<f32>(0.0), fog_to_black);
    return mix(rgb, fog_color, clamp(amount, 0.0, 1.0));
}

fn fogged(color: vec4<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec4<f32> {
    return vec4<f32>(apply_self_legacy_fog(color.rgb, world_position, fog_to_black), color.a);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(lit_color(input.normal, tex) * input.color, input.world_position, false);
}

@fragment
fn fs_mask(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    if tex.a < 0.5 {
        discard;
    }
    return fogged(lit_color(input.normal, tex) * input.color, input.world_position, false);
}

@fragment
fn fs_unlit(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(tex * input.color, input.world_position, false);
}

@fragment
fn fs_unlit_fog_black(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(tex * input.color, input.world_position, true);
}

// Scene depth prepass; see md3.wgsl. Skinning is identical to vs_main so the
// prepass and colour pass rasterize the same surface.
struct PrepassVertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
};

struct PrepassOut {
    @location(0) linear_depth: f32,
    @location(1) motion_vector: vec2<f32>,
    @location(2) reflection_policy: vec4<f32>,
};

fn prepass_output(world_position: vec3<f32>) -> PrepassOut {
    var out: PrepassOut;
    out.linear_depth = distance(world_position, camera.camera_pos_time.xyz);
    out.motion_vector = vec2<f32>(0.0);
    out.reflection_policy = vec4<f32>(0.0, 0.0, 1.0, 2.0 / 3.0);
    return out;
}

@vertex
fn vs_prepass(input: VertexIn, @builtin(instance_index) draw_index: u32) -> PrepassVertexOut {
    let draw = skin_draws[draw_index];
    let model_position = skin_position(
        input.position,
        input.bone_indices,
        input.weights,
        input.weight_count,
        draw.params.x,
    );
    let render_position = jka_to_render(model_to_world(draw, model_position));
    var output: PrepassVertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(render_position, 1.0);
    output.uv = input.uv;
    output.world_position = render_position;
    return output;
}

@fragment
fn fs_prepass(input: PrepassVertexOut) -> PrepassOut {
    return prepass_output(input.world_position);
}

@fragment
fn fs_prepass_mask(input: PrepassVertexOut) -> PrepassOut {
    if textureSample(base_texture, base_sampler, input.uv).a < 0.5 {
        discard;
    }
    return prepass_output(input.world_position);
}
