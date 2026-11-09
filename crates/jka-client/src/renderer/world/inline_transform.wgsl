// Scalar arrays match Rust's packed GpuVertex without vec3 storage padding.
struct Pose {
    axis0: vec4<f32>,
    axis1: vec4<f32>,
    axis2: vec4<f32>,
    origin: vec4<f32>,
    // vertex count, destination prefix in words, vertex stride, normal offset
    params: vec4<u32>,
}
@group(0) @binding(0) var<storage, read> base: array<f32>;
@group(0) @binding(1) var<storage, read_write> vertices: array<f32>;
@group(0) @binding(2) var<uniform> pose: Pose;

fn rotate_render(v: vec3<f32>) -> vec3<f32> {
    let local = vec3<f32>(v.x, -v.z, v.y);
    let world = pose.axis0.xyz * local.x + pose.axis1.xyz * local.y + pose.axis2.xyz * local.z;
    return vec3<f32>(world.x, world.z, -world.y);
}

@compute @workgroup_size(64)
fn transform(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= pose.params.x { return; }
    let src = i * 6u;
    let dst = pose.params.y + i * pose.params.z;
    let p = rotate_render(vec3<f32>(base[src], base[src + 1u], base[src + 2u]))
        + vec3<f32>(pose.origin.x, pose.origin.z, -pose.origin.y);
    let packed = vec3<f32>(base[src + 3u], base[src + 4u], base[src + 5u]);
    var n = packed;
    if (packed.z > 2048.0) {
        var axis = vec3<f32>(packed.xy, 1.0-abs(packed.x)-abs(packed.y));
        if (axis.z < 0.0) {
            axis = vec3<f32>((1.0-abs(axis.yx))*select(vec2<f32>(-1.0),vec2<f32>(1.0),axis.xy>=vec2<f32>(0.0)),axis.z);
        }
        let rotated = rotate_render(axis);
        let oct = rotated / max(dot(abs(rotated),vec3<f32>(1.0)),1e-20);
        n = vec3<f32>(oct.xy,packed.z);
        if (oct.z < 0.0) {
            n = vec3<f32>((1.0-abs(oct.yx))*select(vec2<f32>(-1.0),vec2<f32>(1.0),oct.xy>=vec2<f32>(0.0)),packed.z);
        }
    } else if (packed.z != -1337.0) {
        n = rotate_render(packed);
        let length_sq = dot(n, n);
        if length_sq > 1.0e-20 { n *= 1.0 / sqrt(length_sq); }
    }
    vertices[dst] = p.x;
    vertices[dst + 1u] = p.y;
    vertices[dst + 2u] = p.z;
    let normal = dst + pose.params.w;
    vertices[normal] = n.x;
    vertices[normal + 1u] = n.y;
    vertices[normal + 2u] = n.z;
}
