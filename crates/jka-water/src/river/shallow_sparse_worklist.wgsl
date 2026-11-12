// Sparse tile schedule invocation (B, r_riverTileSchedule 1/2). Not an Unreal
// system. Replaces shallow_sparse_slots.wgsl: one workgroup layer per entry of
// a GPU-built slot worklist, dispatched indirectly with z = entry count
// (jka-river river_tile_schedule.wgsl). Stage arithmetic, addressing and the
// edge policy are unchanged; only which resident slots run differs.
@group(1) @binding(0) var<storage, read> worklist: array<u32>;

fn invocation_cell(id: vec3<u32>) -> vec2<i32> {
    return slot_tiles[worklist[id.z]] * i32(tiles.table.z) + vec2<i32>(id.xy);
}
fn invocation_active(id: vec3<u32>, at: vec2<i32>) -> bool {
    return all(id.xy < vec2<u32>(tiles.table.z)) && in_bounds(at);
}
