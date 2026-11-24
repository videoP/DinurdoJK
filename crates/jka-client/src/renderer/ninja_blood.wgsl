// Draw Blood1 on projected floor triangles using the pool's shared UV basis.
struct BloodUniform {
    view_proj: mat4x4<f32>,
    tangent: vec4<f32>,
    bitangent: vec4<f32>,
    eye: vec4<f32>,
    ambient: vec4<f32>,
    directed: vec4<f32>,
    direction: vec4<f32>,
    pigment: vec4<f32>,
}
@group(0) @binding(0) var<uniform> blood: BloodUniform;
@group(0) @binding(1) var blood_velocity_density: texture_2d<f32>;
@group(0) @binding(2) var blood_clamp: sampler;
struct BloodVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
}
@vertex
fn blood_vertex(@location(0) world: vec3<f32>, @location(1) uv: vec2<f32>) -> BloodVertex {
    return BloodVertex(blood.view_proj*vec4<f32>(world,1.0),uv,world);
}
@fragment
fn blood_fragment(input: BloodVertex, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let vertex_normal = normalize(cross(blood.bitangent.xyz, blood.tangent.xyz));
    let normal_ue = vec3<f32>(vertex_normal.x, -vertex_normal.z, vertex_normal.y);
    let face_sign = select(-1.0, 1.0, front);
    let surface = ninja_blood_surface(input.uv, normal_ue, face_sign);
    let opacity = clamp(surface.opacity, 0.0, 1.0) * blood.pigment.w;
    if opacity < 0.01 { discard; }
    let tangent = blood.tangent.xyz;
    let bitangent = blood.bitangent.xyz;
    let tangent_ue = vec3<f32>(tangent.x, -tangent.z, tangent.y);
    let bitangent_ue = vec3<f32>(bitangent.x, -bitangent.z, bitangent.y);
    let world_normal_ue = fnl_blood_material_world_normal(
        surface.normal_ue, normal_ue, tangent_ue, bitangent_ue, face_sign);
    let n = normalize(vec3<f32>(world_normal_ue.x, world_normal_ue.z, -world_normal_ue.y));

    // Copyright 2022 Wolfire Games LLC, Apache-2.0.
    // Data/GLSL/decals.glsl and splatdecal.frag: pigment and wet specular.
    let pigment = blood.pigment.rgb;
    let wetness = clamp(blood.tangent.w, 0.0, 1.0);
    var color = pigment * blood.ambient.rgb;
    if dot(blood.direction.xyz, blood.direction.xyz) > 0.0 {
        let l = normalize(blood.direction.xyz);
        let ndotl = max(dot(n, l), 0.0);
        color += pigment * blood.directed.rgb * ndotl;
        let view = normalize(blood.eye.xyz - input.world);
        if dot(view + l, view + l) > 1e-8 && ndotl > 0.0 {
            let h = normalize(view + l);
            let spec = min(1.0, pow(max(0.0, dot(n, h)), 850.0) * pow(20.0, wetness) * 0.5);
            color += vec3<f32>(spec) * blood.directed.rgb;
        }
    }
    return vec4<f32>(color, opacity);
}
