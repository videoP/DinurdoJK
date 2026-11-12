// Native sparse invocation (A): one workgroup layer per resident slot.
// Appended to shallow.wgsl + shallow_sparse.wgsl. The worklist variant
// (shallow_sparse_worklist.wgsl) replaces only these two functions.

// One workgroup layer per resident slot: dispatch (tile/8, tile/8, slots).
fn invocation_cell(id: vec3<u32>) -> vec2<i32> {
    let slot = min(id.z, max(tiles.counts.x, 1u) - 1u);
    return slot_tiles[slot] * i32(tiles.table.z) + vec2<i32>(id.xy);
}
fn invocation_active(id: vec3<u32>, at: vec2<i32>) -> bool {
    return id.z < tiles.counts.x && all(id.xy < vec2<u32>(tiles.table.z)) && in_bounds(at);
}

// Per-slot occupancy after a frame: any cell that is not dry, still water at
// its bed. The host keeps a guard ring of resident tiles around these.
var<workgroup> group_occupied: atomic<u32>;
@compute @workgroup_size(8, 8)
fn tile_occupancy(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    if lane == 0u { atomicStore(&group_occupied, 0u); }
    workgroupBarrier();
    let at = invocation_cell(id);
    if invocation_active(id, at) {
        let i = index(at);
        if any(water[i] != vec4<f32>(geometry[i].x, 0.0, 0.0, 0.0)) {
            atomicOr(&group_occupied, 1u);
        }
    }
    workgroupBarrier();
    if lane == 0u && atomicLoad(&group_occupied) != 0u && id.z < tiles.counts.x {
        atomicOr(&occupancy[id.z], 1u);
    }
}
