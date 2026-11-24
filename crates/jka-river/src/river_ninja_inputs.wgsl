// DinurdoJK native input helpers (no Fluid Ninja graph content; the
// FNL spline-slot substitution lives in the private jka-ninja crate).

// A half-open destination rectangle and its caller-proven read halo in
// native cells (not FNL cells). Map conservatively to the native 16-cell
// occupancy grid. Never identify independent FNL and hydraulic indices.
// Caller supplies a clipped nonempty rectangle and positive dimensions.
fn ninja_native_block_rect(lo: vec2<u32>, hi: vec2<u32>, dst: vec2<u32>, native_size: vec2<u32>, grid: vec2<u32>, halo: vec2<u32>) -> vec4<u32> {
    let a = lo * native_size / dst;
    let b = (hi * native_size + dst - vec2<u32>(1u)) / dst;
    let lower = a - min(a, halo);
    let upper = b + min(native_size - b, halo);
    let block = vec2<u32>(16u);
    return vec4<u32>(min(lower / block, grid), min((upper + block - vec2<u32>(1u)) / block, grid));
}
