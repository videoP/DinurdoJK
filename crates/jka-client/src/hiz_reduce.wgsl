@group(0) @binding(0) var source_depth: texture_2d<f32>;
@group(0) @binding(1) var target_depth: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dst_size = textureDimensions(target_depth);
    if (gid.x >= dst_size.x || gid.y >= dst_size.y) {
        return;
    }

    let src_size = textureDimensions(source_depth);
    let base = vec2<u32>(gid.xy) * 2u;
    let max_coord = max(src_size, vec2<u32>(1u)) - vec2<u32>(1u);
    let p00 = min(base, max_coord);
    let p10 = min(base + vec2<u32>(1u, 0u), max_coord);
    let p01 = min(base + vec2<u32>(0u, 1u), max_coord);
    let p11 = min(base + vec2<u32>(1u, 1u), max_coord);

    let d00 = textureLoad(source_depth, vec2<i32>(p00), 0).x;
    let d10 = textureLoad(source_depth, vec2<i32>(p10), 0).x;
    let d01 = textureLoad(source_depth, vec2<i32>(p01), 0).x;
    let d11 = textureLoad(source_depth, vec2<i32>(p11), 0).x;
    let maximum = max(max(d00, d10), max(d01, d11));
    textureStore(target_depth, vec2<i32>(gid.xy), vec4<f32>(maximum, 0.0, 0.0, 0.0));
}
