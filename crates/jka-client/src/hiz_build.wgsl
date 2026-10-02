// Hi-Z pyramid, mip 0: reduces the camera depth buffer into a texture whose
// sides are the previous power of two of the viewport.
//
// Camera depth is reversed-Z (near -> 1, far -> 0), so the conservative
// occluder depth over a region is the MINIMUM (the farthest surface), and an
// undrawn pixel (cleared to 0) correctly poisons the region to "not occluded".
//
// Every pyramid texel owns the exact range of source pixels its UV interval
// overlaps (1..3 pixels per axis, because the ratio is in [1, 2)). Together
// with exact 2x2 reductions above this mip, texel i of every level covers
// exactly UV [i/n, (i+1)/n), which is what the culling shader indexes with.
@group(0) @binding(0) var source_depth: texture_depth_2d;
@group(0) @binding(1) var target_depth: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dst_size = textureDimensions(target_depth);
    if (gid.x >= dst_size.x || gid.y >= dst_size.y) {
        return;
    }

    let src_size = textureDimensions(source_depth);
    let x0 = (gid.x * src_size.x) / dst_size.x;
    let y0 = (gid.y * src_size.y) / dst_size.y;
    let x1 = min(((gid.x + 1u) * src_size.x + dst_size.x - 1u) / dst_size.x, src_size.x);
    let y1 = min(((gid.y + 1u) * src_size.y + dst_size.y - 1u) / dst_size.y, src_size.y);

    var farthest = 1.0;
    for (var y = y0; y < y1; y += 1u) {
        for (var x = x0; x < x1; x += 1u) {
            farthest = min(farthest, textureLoad(source_depth, vec2<i32>(i32(x), i32(y)), 0));
        }
    }
    textureStore(target_depth, vec2<i32>(gid.xy), vec4<f32>(farthest, 0.0, 0.0, 0.0));
}
