// `r_oceanPixelDetail` 5 (adaptive 2x2): which 16x16-pixel tiles get the full material, from the 2x2 reduced shading.
// One workgroup per tile (8x8 reduced texels, one per 2x2 block). A full-resolution pixel's bilinear reconstruction
// reads the reduced texels around its block's texel, so a block needs the full material where that 3x3 neighbourhood
// is not close to linear. The decision is made per tile, not per block: the GPU shades 32 pixels together, and one
// full block among them runs the material for all 32. A tile goes full for any water edge or depth step (lines that
// would show), or for luminance curvature in at least DENSE_BLOCKS of its blocks (foam, crest lines, close detail);
// sparse glints alone stay reconstructed, as in the plain 2x2 mode.

@group(0) @binding(0) var reduced_color: texture_2d<f32>;
@group(0) @binding(1) var reduced_depth: texture_depth_2d;
@group(0) @binding(2) var<storage, read_write> tiles: array<u32>;
// x: luminance curvature share (`r_oceanPixelContrast`). y: use Coast's material flags in alpha.
@group(0) @binding(3) var<uniform> params: vec4<f32>;

const DENSE_BLOCKS: u32 = 8u;

var<workgroup> reasons: array<atomic<u32>, 5>;
// The 8x8 tile plus its one-texel border. Luminance, depth and eligibility
// are read from the reduced targets once per tile, then shared by its lanes.
var<workgroup> neighbourhood: array<vec3<f32>, 100>;

// Why one block needs the full material: 0 it does not, 1 a texel without water, 2 a depth step the reconstruction
// would reject a neighbour across, 3 luminance curving past the threshold.
fn block_reason(centre: u32) -> u32 {
    var lum: array<f32, 9>;
    var depth: array<f32, 9>;
    for (var j = 0u; j < 3u; j++) {
        for (var i = 0u; i < 3u; i++) {
            let value = neighbourhood[centre - 11u + j * 10u + i];
            if value.z < 0.5 { return 1u; }
            if params.y > 0.5 && value.z > 1.5 { return 4u; }
            lum[j * 3u + i] = value.x;
            depth[j * 3u + i] = value.y;
        }
    }
    // The reconstruction weighs a neighbour by exp(-600 |depth step|): past 1/600 it starts leaving one out.
    for (var n = 0; n < 9; n++) {
        if abs(depth[n] - depth[4]) * 600.0 > 0.5 { return 2u; }
    }
    let pairs = array<vec2<i32>, 4>(vec2<i32>(3, 5), vec2<i32>(1, 7), vec2<i32>(0, 8), vec2<i32>(2, 6));
    for (var k = 0; k < 4; k++) {
        let a = pairs[k].x;
        let b = pairs[k].y;
        let level = lum[a] + lum[b] + 2.0 * lum[4];
        if abs(lum[a] + lum[b] - 2.0 * lum[4]) > params.x * level + 1e-6 { return 3u; }
    }
    return 0u;
}

@compute @workgroup_size(8, 8)
fn classify(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) local: u32,
        @builtin(workgroup_id) group: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    if local < 5u {
        atomicStore(&reasons[local], 0u);
    }
    let dim = vec2<i32>(textureDimensions(reduced_color));
    let tile_origin = vec2<i32>(group.xy * 8u) - 1;
    for (var i = local; i < 100u; i += 64u) {
        let offset = vec2<i32>(i32(i % 10u), i32(i / 10u));
        let at = clamp(tile_origin + offset, vec2<i32>(0), dim - 1);
        let shaded = textureLoad(reduced_color, at, 0);
        let depth = textureLoad(reduced_depth, at, 0);
        neighbourhood[i] = vec3<f32>(dot(shaded.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), depth, shaded.a);
    }
    workgroupBarrier();
    let centre = 11u + (local / 8u) * 10u + local % 8u;
    // Blocks without water at all (the tile's dry part) need nothing.
    if all(vec2<i32>(id.xy) < dim) && neighbourhood[centre].z >= 0.5 {
        atomicAdd(&reasons[block_reason(centre)], 1u);
    }
    workgroupBarrier();
    if local == 0u {
        var reason = 0u;
        if atomicLoad(&reasons[3]) >= DENSE_BLOCKS { reason = 3u; }
        if atomicLoad(&reasons[2]) > 0u { reason = 2u; }
        if atomicLoad(&reasons[1]) > 0u { reason = 1u; }
        if params.y > 0.5 && (atomicLoad(&reasons[4]) > 0u
            || atomicLoad(&reasons[0]) + atomicLoad(&reasons[1]) + atomicLoad(&reasons[2]) + atomicLoad(&reasons[3]) == 0u) {
            reason = 4u;
        }
        tiles[group.y * groups.x + group.x] = reason;
    }
}
