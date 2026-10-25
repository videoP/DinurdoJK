// Stage-1 candidate occlusion probe. NOT YET DISPATCHED by Renderer.
// See OCCLUSION_HANDOFF.md for GPU->CPU latency and draw integration.
// A result of 1 means a conservative raster occlusion proof; 0 means
// potentially visible / indeterminate (including missing depth).
//
// CPU supplies renderer-space bounds. A node/group can be represented by
// one box; MD3 surfaces must use the enclosing prop bounds, not per-stage
// UV/material metadata. Only opaque+masked depth-writing world geometry may
// seed the pyramid. A transparently composited FX emitter does not occlude.
struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};
struct ProbeBox {
    minimum: vec4<f32>,
    maximum: vec4<f32>,
};
struct Settings {
    count: u32,
    mip_count: u32,
    _padding0: u32,
    _padding1: u32,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<storage, read> boxes: array<ProbeBox>;
@group(1) @binding(1) var<storage, read_write> is_occluded: array<u32>;
@group(1) @binding(2) var pyramid: texture_2d<f32>;
@group(1) @binding(3) var<uniform> settings: Settings;

fn corner(bounds: ProbeBox, index: u32) -> vec3<f32> {
    return vec3<f32>(
        select(bounds.minimum.x, bounds.maximum.x, (index & 1u) != 0u),
        select(bounds.minimum.y, bounds.maximum.y, (index & 2u) != 0u),
        select(bounds.minimum.z, bounds.maximum.z, (index & 4u) != 0u),
    );
}

fn hidden(bounds: ProbeBox) -> bool {
    var uv_min = vec2<f32>(1.0);
    var uv_max = vec2<f32>(0.0);
    var near_depth = 0.0;
    for (var i = 0u; i < 8u; i += 1u) {
        let projected = camera.view_proj * vec4<f32>(corner(bounds, i), 1.0);
        // A box intersecting the near plane is unbounded in screen space.
        if (projected.w <= 0.0001 || projected.z > projected.w || projected.z < 0.0) {
            return false;
        }
        let ndc = projected.xy / projected.w;
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        uv_min = min(uv_min, uv);
        uv_max = max(uv_max, uv);
        near_depth = max(near_depth, projected.z / projected.w);
    }
    // Off-screen boxes are not occlusion proofs; frustum culling owns those.
    if (uv_max.x < 0.0 || uv_max.y < 0.0 || uv_min.x > 1.0 || uv_min.y > 1.0) {
        return false;
    }
    uv_min = clamp(uv_min, vec2<f32>(0.0), vec2<f32>(1.0));
    uv_max = clamp(uv_max, vec2<f32>(0.0), vec2<f32>(1.0));
    let dimensions = vec2<f32>(textureDimensions(pyramid, 0));
    let footprint = max((uv_max.x - uv_min.x) * dimensions.x,
                        (uv_max.y - uv_min.y) * dimensions.y);
    let mip = min(u32(max(0.0, ceil(log2(max(footprint, 1.0))))),
                  max(settings.mip_count, 1u) - 1u);
    let dims = textureDimensions(pyramid, i32(mip));
    let last = max(dims, vec2<u32>(1u)) - vec2<u32>(1u);
    let c0 = min(vec2<u32>(uv_min * vec2<f32>(dims)), last);
    let c1 = min(vec2<u32>(vec2<f32>(uv_min.x, uv_max.y) * vec2<f32>(dims)), last);
    let c2 = min(vec2<u32>(vec2<f32>(uv_max.x, uv_min.y) * vec2<f32>(dims)), last);
    let c3 = min(vec2<u32>(uv_max * vec2<f32>(dims)), last);
    let farthest_occluder = min(
        min(textureLoad(pyramid, vec2<i32>(c0), i32(mip)).x,
            textureLoad(pyramid, vec2<i32>(c1), i32(mip)).x),
        min(textureLoad(pyramid, vec2<i32>(c2), i32(mip)).x,
            textureLoad(pyramid, vec2<i32>(c3), i32(mip)).x));
    // Reversed Z: the minimum depth within a footprint is the farthest.
    // A hole in the depth buffer reads zero and conservatively keeps the box.
    return near_depth < farthest_occluder * 0.9998;
}

@compute @workgroup_size(64, 1, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= settings.count || i >= arrayLength(&boxes) || i >= arrayLength(&is_occluded)) {
        return;
    }
    is_occluded[i] = u32(hidden(boxes[i]));
}
