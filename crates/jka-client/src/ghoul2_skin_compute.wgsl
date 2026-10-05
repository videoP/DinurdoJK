// RT-shadow-only compute skinning path. The deformation math is a direct port
// of ghoul2_skin.wgsl, which itself mirrors OpenJK RB_SurfaceGhoul weighting.
// Unlike the normal RT-off path, this materializes the final world-space
// DynamicModelVertex once so both rasterization and hardware BLAS builds consume
// the same skinned result.

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
    classic_sun_directed: vec4<f32>,
    classic_sun_direction: vec4<f32>,
    uv_xform: vec4<f32>,
    spec_light: vec4<f32>,
    spec_viewer: vec4<f32>,
    jiggle0: vec4<f32>,
    jiggle1: vec4<f32>,
    jiggle2: vec4<f32>,
    jiggle3: vec4<f32>,
};
// q3 RB_CalcSpecularAlpha: (reflected light . viewer)^4, all in JKA world space.
fn specular_alpha(draw: SkinDraw, world: vec3<f32>, world_normal: vec3<f32>) -> f32 {
    if (draw.spec_light.w == 0.0) {
        return 1.0;
    }
    let normal = normalize(world_normal);
    let light_dir = normalize(draw.spec_light.xyz - world);
    let reflected = normal * (2.0 * dot(normal, light_dir)) - light_dir;
    let viewer = normalize(draw.spec_viewer.xyz - world);
    let l = dot(reflected, viewer);
    if (l < 0.0) {
        return 0.0;
    }
    let l2 = l * l;
    return min(l2 * l2, 1.0);
}


// Storage-friendly repack of Ghoul2GpuVertex. The normal raster path keeps its
// original packed vertex format; this copy exists lazily only while RT shadows
// are active.
struct InputVertex {
    position_uv_x: vec4<f32>,
    normal_uv_y: vec4<f32>,
    bone_indices: vec4<u32>,
    weights: vec4<f32>,
    params: vec4<u32>,
};

@group(0) @binding(0) var<storage, read> input_vertices: array<InputVertex>;
@group(0) @binding(1) var<storage, read> draw_indices: array<u32>;
@group(1) @binding(0) var<storage, read> bones: array<BoneMatrix>;
@group(1) @binding(1) var<storage, read> skin_draws: array<SkinDraw>;
// Packed DynamicModelVertex words: position3, normal3, uv2, color4 = 12 words.
@group(1) @binding(2) var<storage, read_write> output_words: array<u32>;

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
        // Exact OpenJK/RB_SurfaceGhoul optimized two-weight form, copied from
        // the existing Ghoul2 vertex shader.
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

fn jiggle_offset(draw: SkinDraw, region: u32) -> vec3<f32> {
    switch region {
        case 0u: { return draw.jiggle0.xyz; }
        case 1u: { return draw.jiggle1.xyz; }
        case 2u: { return draw.jiggle2.xyz; }
        case 3u: { return draw.jiggle3.xyz; }
        default: { return vec3<f32>(0.0); }
    }
}

fn jiggle_effective_weight(draw: SkinDraw, weight: f32, coord: f32) -> f32 {
    let overall = draw.jiggle0.w;
    if (abs(coord) < 2.0) {
        let vertical = smoothstep(-0.70 + draw.jiggle3.w, -0.15 + draw.jiggle3.w, coord);
        return weight * overall * draw.jiggle2.w * vertical;
    }
    return weight * overall * draw.jiggle1.w;
}

fn model_to_world(draw: SkinDraw, model_position: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        draw.origin.x + draw.axis0.x * model_position.x + draw.axis1.x * model_position.y + draw.axis2.x * model_position.z,
        draw.origin.y + draw.axis0.y * model_position.x + draw.axis1.y * model_position.y + draw.axis2.y * model_position.z,
        draw.origin.z + draw.axis0.z * model_position.x + draw.axis1.z * model_position.y + draw.axis2.z * model_position.z,
    );
}

fn jka_to_render(world: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(world.x, world.z, -world.y);
}

fn write_f32(word: u32, value: f32) {
    output_words[word] = bitcast<u32>(value);
}

fn write_vertex(
    output_vertex: u32,
    position: vec3<f32>,
    normal: vec3<f32>,
    uv: vec2<f32>,
    color: vec4<f32>,
) {
    let base = output_vertex * 12u;
    write_f32(base + 0u, position.x);
    write_f32(base + 1u, position.y);
    write_f32(base + 2u, position.z);
    write_f32(base + 3u, normal.x);
    write_f32(base + 4u, normal.y);
    write_f32(base + 5u, normal.z);
    write_f32(base + 6u, uv.x);
    write_f32(base + 7u, uv.y);
    write_f32(base + 8u, color.x);
    write_f32(base + 9u, color.y);
    write_f32(base + 10u, color.z);
    write_f32(base + 11u, color.w);
}

@compute @workgroup_size(64, 1, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let vertex_index = gid.x;
    let batch_instance = gid.y;
    if vertex_index >= arrayLength(&input_vertices) || batch_instance >= arrayLength(&draw_indices) {
        return;
    }

    let draw_index = draw_indices[batch_instance];
    let draw = skin_draws[draw_index];
    let input = input_vertices[vertex_index];
    let bone_base = draw.params.x;
    var model_position = skin_position(
        input.position_uv_x.xyz,
        input.bone_indices,
        input.weights,
        input.params.x,
        bone_base,
    );
    let jiggle_weight = bitcast<f32>(input.params.z);
    let jiggle_coord = bitcast<f32>(input.params.w);
    if (jiggle_weight > 0.0 && input.params.y < 4u) {
        let effective_weight = jiggle_effective_weight(draw, jiggle_weight, jiggle_coord);
        model_position += jiggle_offset(draw, input.params.y) * effective_weight;
    }

    // Match OpenJK's fast Ghoul2 normal path exactly: weight-0's bone only.
    let model_normal = transform_vector(
        bones[bone_base + input.bone_indices.x],
        input.normal_uv_y.xyz,
    );
    let world = model_to_world(draw, model_position);
    let world_normal = vec3<f32>(
        draw.axis0.x * model_normal.x + draw.axis1.x * model_normal.y + draw.axis2.x * model_normal.z,
        draw.axis0.y * model_normal.x + draw.axis1.y * model_normal.y + draw.axis2.y * model_normal.z,
        draw.axis0.z * model_normal.x + draw.axis1.z * model_normal.y + draw.axis2.z * model_normal.z,
    );
    let render_position = jka_to_render(world);
    let render_normal = normalize(vec3<f32>(world_normal.x, world_normal.z, -world_normal.y));

    var draw_color = draw.color;
    if draw.params.y != 0u {
        var incoming = 0.0;
        let light_dir = draw.classic_direction.xyz;
        let light_len_sq = dot(light_dir, light_dir);
        if light_len_sq > 1.0e-8 {
            incoming = max(dot(render_normal, light_dir * inverseSqrt(light_len_sq)), 0.0);
        }
        // Runtime sun (entity sun lighting): zero radiance when the feature is off.
        var sun_incoming = 0.0;
        let sun_dir = draw.classic_sun_direction.xyz;
        let sun_len_sq = dot(sun_dir, sun_dir);
        if sun_len_sq > 1.0e-8 {
            sun_incoming = max(dot(render_normal, sun_dir * inverseSqrt(sun_len_sq)), 0.0);
        }
        let lighting = clamp(
            (draw.classic_ambient.xyz
                + incoming * draw.classic_directed.xyz
                + sun_incoming * draw.classic_sun_directed.xyz) / 255.0,
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        draw_color = vec4<f32>(draw.color.rgb * lighting, draw.color.a);
    }

    draw_color.a = draw_color.a * specular_alpha(draw, world, world_normal);
    let uv = vec2<f32>(input.position_uv_x.w, input.normal_uv_y.w) * draw.uv_xform.xy + draw.uv_xform.zw;
    write_vertex(draw.params.z + vertex_index, render_position, render_normal, uv, draw_color);
}
