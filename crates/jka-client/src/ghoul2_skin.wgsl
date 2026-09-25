struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};

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
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
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

    let world = vec3<f32>(
        draw.origin.x + draw.axis0.x * model_position.x + draw.axis1.x * model_position.y + draw.axis2.x * model_position.z,
        draw.origin.y + draw.axis0.y * model_position.x + draw.axis1.y * model_position.y + draw.axis2.y * model_position.z,
        draw.origin.z + draw.axis0.z * model_position.x + draw.axis1.z * model_position.y + draw.axis2.z * model_position.z,
    );
    let world_normal = vec3<f32>(
        draw.axis0.x * model_normal.x + draw.axis1.x * model_normal.y + draw.axis2.x * model_normal.z,
        draw.axis0.y * model_normal.x + draw.axis1.y * model_normal.y + draw.axis2.y * model_normal.z,
        draw.axis0.z * model_normal.x + draw.axis1.z * model_normal.y + draw.axis2.z * model_normal.z,
    );

    // Same JKA -> renderer conversion as scene::render_position([x,y,z]).
    let render_position = vec3<f32>(world.x, world.z, -world.y);
    let render_normal = normalize(vec3<f32>(world_normal.x, world_normal.z, -world_normal.y));

    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(render_position, 1.0);
    output.uv = input.uv;
    output.normal = render_normal;
    output.color = draw.color;
    return output;
}

fn lit_color(normal: vec3<f32>, tex: vec4<f32>) -> vec4<f32> {
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
