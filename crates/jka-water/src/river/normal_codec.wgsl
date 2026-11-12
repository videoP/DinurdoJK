// Octahedral unit-normal codec for the sparse river pool (r_riverStorageMode 2;
// not Unreal). One u32: pack2x16snorm of the octahedral square, ~3e-5 per
// component, so about 1e-4 rad of error. +Z (the dry normal) packs to 0u and
// decodes to exactly +Z; the tile schedule compares the packed word.
fn river_pack_normal(n: vec3<f32>) -> u32 {
    var p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if n.z < 0.0 {
        p = (vec2<f32>(1.0) - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
    }
    return pack2x16snorm(p);
}
fn river_unpack_normal(packed: u32) -> vec3<f32> {
    if packed == 0u { return vec3<f32>(0.0, 0.0, 1.0); }
    let e = unpack2x16snorm(packed);
    var n = vec3<f32>(e, 1.0 - abs(e.x) - abs(e.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}
