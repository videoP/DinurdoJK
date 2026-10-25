// JKA integration boundary, evaluated in native map units before the preserved
// Blue Water material shades a fragment. The upstream shader file is unchanged.
struct JkaWaterBoundary {
    bounds: vec4<f32>,
    // Bottom relative to the water plane in metres; y enables floor clipping.
    depth: vec4<f32>,
    // triangle count, sky segment count, PVS enabled, unused.
    counts: vec4<u32>,
    // JKA units per metre and shoreline tolerance in JKA units.
    scale: vec4<f32>,
    tiles: vec4<u32>,
    pvs: array<vec4<u32>, 8>,
    triangles: array<vec4<f32>, 512>,
    sky: array<vec4<f32>, 16>,
    candidates: array<vec4<u32>, 128>,
}
@group(1) @binding(16) var<uniform> jka_boundary: JkaWaterBoundary;

fn jka_water_column_contains(position: vec3<f32>) -> bool {
    return (jka_boundary.depth.y < 0.5 || position.y >= jka_boundary.depth.x)
        && jka_water_contains(position.xz);
}

fn jka_water_contains(grid: vec2<f32>) -> bool {
    // Diagnostic modes are runtime uniform gates (no change to the upstream shader).
    if jka_boundary.counts.w == 2u { return true; }
    let p = grid * jka_boundary.scale.x;
    let bounds = jka_boundary.bounds;
    if any(p < bounds.xy) || any(p > bounds.zw) {
        // Exterior strips and corners need the same sky contacts as JKA.
        var found_x = false;
        var found_z = false;
        let xs = select(select(0u, 2u, p.x > bounds.z), 1u, p.x < bounds.x);
        let zs = select(select(0u, 4u, p.y > bounds.w), 3u, p.y < bounds.y);
        let xc = select(bounds.z, bounds.x, xs == 1u);
        let zc = select(bounds.w, bounds.y, zs == 3u);
        for (var i = 0u; i < jka_boundary.counts.y; i += 1u) {
            let s = jka_boundary.sky[i];
            let side = u32(s.w);
            if ((side == 1u && p.x <= s.x + 2.0 && p.y >= s.y && p.y <= s.z)
                || (side == 2u && p.x >= s.x - 2.0 && p.y >= s.y && p.y <= s.z)
                || (side == 3u && p.y <= s.x + 2.0 && p.x >= s.y && p.x <= s.z)
                || (side == 4u && p.y >= s.x - 2.0 && p.x >= s.y && p.x <= s.z)) {
                return true;
            }
            if side == xs && zc >= s.y - 2.0 && zc <= s.z + 2.0 { found_x = true; }
            if side == zs && xc >= s.y - 2.0 && xc <= s.z + 2.0 { found_z = true; }
        }
        return xs != 0u && zs != 0u && found_x && found_z;
    }
    let uv = clamp((p - bounds.xy) / max(bounds.zw - bounds.xy, vec2<f32>(1e-4)),
        vec2<f32>(0.0), vec2<f32>(0.999999));
    if jka_boundary.counts.z != 0u {
        let xy = vec2<u32>(floor(uv * 32.0));
        let row = jka_boundary.pvs[xy.y >> 2u][xy.y & 3u];
        if (row & (1u << xy.x)) == 0u { return false; }
    }
    if jka_boundary.counts.w == 1u || jka_boundary.counts.x == 0u { return true; }
    let xy = vec2<u32>(floor(uv * 8.0));
    let tile = xy.y * 8u + xy.x;
    let code = (jka_boundary.tiles[tile >> 4u] >> ((tile & 15u) * 2u)) & 3u;
    if code == 1u { return false; }
    if code == 2u { return true; }
    let tolerance = jka_boundary.scale.y;
    for (var i = 0u; i < jka_boundary.counts.x; i += 1u) {
        if jka_boundary.counts.w == 3u {
            let word = tile * 8u + (i >> 5u);
            if (jka_boundary.candidates[word >> 2u][word & 3u] & (1u << (i & 31u))) == 0u { continue; }
        }
        let t0 = jka_boundary.triangles[i * 2u];
        let t1 = jka_boundary.triangles[i * 2u + 1u];
        let a = t0.xy;
        let b = t0.zw;
        let c = t1.xy;
        if any(p < min(a, min(b, c)) - vec2<f32>(tolerance))
            || any(p > max(a, max(b, c)) + vec2<f32>(tolerance)) { continue; }
        let ab = b - a;
        let bc = c - b;
        let ca = a - c;
        let e0 = (ab.x * (p.y - a.y) - ab.y * (p.x - a.x)) / max(length(ab), 1e-4);
        let e1 = (bc.x * (p.y - b.y) - bc.y * (p.x - b.x)) / max(length(bc), 1e-4);
        let e2 = (ca.x * (p.y - c.y) - ca.y * (p.x - c.x)) / max(length(ca), 1e-4);
        if all(vec3<f32>(e0, e1, e2) >= vec3<f32>(-tolerance))
            || all(vec3<f32>(e0, e1, e2) <= vec3<f32>(tolerance)) { return true; }
    }
    return false;
}
