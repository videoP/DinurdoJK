// Screen bounds of glass coverage, so the edge field and blur pyramid only run
// where glass_shade can read them. Same-frame and GPU-only: no readback lag.
// result[0] = edge indirect dispatch (x, y, 1), result[1] = half-res edge region
// [x0, y0, x1, y1), result[2] = full-res blur rectangle [x0, y0, x1, y1),
// result[3].xy = full resolution (mip sizes for the blur vertex shader).
struct Params {
    full: vec2<u32>,
    half: vec2<u32>,
    edge_pad: u32,
    blur_pad: u32,
    use_depth: u32,
    _pad: u32,
};
@group(0) @binding(0) var mask: texture_2d<f32>;
@group(0) @binding(1) var scene_depth: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> accum: array<atomic<u32>, 8>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<u32>, 4>;
@group(0) @binding(4) var<uniform> params: Params;

var<workgroup> local: array<atomic<u32>, 5>;

@compute @workgroup_size(16, 16)
fn reduce(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) index: u32) {
    if (index < 5u) { atomicStore(&local[index], 0u); }
    workgroupBarrier();
    if (all(gid.xy < params.full)) {
        let value = textureLoad(mask, vec2<i32>(gid.xy), 0);
        var covered = value.w > 0.0;
        if (covered && params.use_depth != 0u) {
            // Linear view depth, as in glass_edges.wgsl. Sky is 1e6. Only glass
            // clearly behind the opaque scene is dropped; ties stay covered.
            let glass = value.z / max(value.w, 0.001);
            let scene = textureLoad(scene_depth, vec2<i32>(gid.xy), 0).r;
            covered = scene >= glass * 0.98 - 4.0;
        }
        if (covered) {
            atomicMax(&local[0], 1u);
            atomicMax(&local[1], gid.x);
            atomicMax(&local[2], gid.y);
            atomicMax(&local[3], params.full.x - 1u - gid.x);
            atomicMax(&local[4], params.full.y - 1u - gid.y);
        }
    }
    workgroupBarrier();
    if (index == 0u && atomicLoad(&local[0]) != 0u) {
        atomicMax(&accum[0], 1u);
        atomicMax(&accum[1], atomicLoad(&local[1]));
        atomicMax(&accum[2], atomicLoad(&local[2]));
        atomicMax(&accum[3], atomicLoad(&local[3]));
        atomicMax(&accum[4], atomicLoad(&local[4]));
    }
}

fn padded(lo: u32, hi: u32, pad: u32, limit: u32) -> vec2<u32> {
    return vec2<u32>(select(0u, lo - pad, lo > pad), min(hi + 1u + pad, limit));
}

@compute @workgroup_size(1)
fn finalize() {
    result[3] = vec4<u32>(params.full, 0u, 0u);
    if (atomicLoad(&accum[0]) == 0u) {
        result[0] = vec4<u32>(0u, 0u, 1u, 0u);
        result[1] = vec4<u32>(0u);
        result[2] = vec4<u32>(0u);
        return;
    }
    let lo = vec2<u32>(params.full.x - 1u - atomicLoad(&accum[3]), params.full.y - 1u - atomicLoad(&accum[4]));
    let hi = vec2<u32>(atomicLoad(&accum[1]), atomicLoad(&accum[2]));
    let edge_x = padded(lo.x, hi.x, params.edge_pad, params.full.x);
    let edge_y = padded(lo.y, hi.y, params.edge_pad, params.full.y);
    // Half-res texel h covers full pixels 2h and 2h + 1.
    let region = vec4<u32>(edge_x.x / 2u, edge_y.x / 2u,
        min((edge_x.y + 1u) / 2u, params.half.x), min((edge_y.y + 1u) / 2u, params.half.y));
    result[0] = vec4<u32>((region.z - region.x + 7u) / 8u, (region.w - region.y + 7u) / 8u, 1u, 0u);
    result[1] = region;
    let blur_x = padded(lo.x, hi.x, params.blur_pad, params.full.x);
    let blur_y = padded(lo.y, hi.y, params.blur_pad, params.full.y);
    result[2] = vec4<u32>(blur_x.x, blur_y.x, blur_x.y, blur_y.y);
}
