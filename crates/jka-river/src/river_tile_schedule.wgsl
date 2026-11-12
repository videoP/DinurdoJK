// Sparse river tile schedule (r_riverTileSchedule). Not an Unreal system and
// NOT local time stepping: every scheduled tile runs every native stage at the
// native dt. Mode 2 ("dormant-dry") only leaves out resident tiles on which a
// whole solver frame would be the identity, so the pool after a frame is the
// one the native all-slot dispatch writes. See LTS_DESIGN.md (Patch20).
//
// A slot is DIRTY unless every cell holds the dry fixed point in every buffer
// the stages write: both ping-pong states = (bed, 0, 0, 0), PreSim history =
// bed, PrepareSurface output = (bed, 0, 0, 0), normals = +Z and no moving-body
// collision velocity. NaN compares unequal, so it is dirty. A slot is
// scheduled when any resident tile within `halo` tiles (Chebyshev) is dirty,
// or it lies within `halo` of a forcing rectangle (source box, CFS inlet, body
// footprints): those write water into cells that still read as dry here.
// Non-resident tiles read as walls of dry water and never change, so they
// never wake a slot. Pool index = slot * tile^2 + local_y * tile + local_x.
// History (binding 4) and normals (6) come from the r_riverStorageMode
// prelude (river_storage.rs).

struct Schedule {
    counts: vec4<u32>, // resident slots, tile size, halo (tiles), forcing rects
    table: vec4<u32>,  // table width, table height (tiles), skip dormant (0 = run all), unused
    seeds: array<vec4<i32>, 40>, // forcing rects in tiles, inclusive x0 y0 x1 y1
}
@group(0) @binding(0) var<uniform> schedule: Schedule;
@group(0) @binding(1) var<storage, read> state_a: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> state_b: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> geometry: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> surface: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> dirty: array<atomic<u32>>;
@group(0) @binding(8) var<storage, read> tile_slots: array<i32>; // -1: not resident
@group(0) @binding(9) var<storage, read> slot_tiles: array<vec2<i32>>;
@group(0) @binding(10) var<storage, read_write> worklist: array<u32>;
// Indirect dispatch arguments: tile/8, tile/8, scheduled slots.
@group(0) @binding(11) var<storage, read_write> indirect_args: array<atomic<u32>>;

var<workgroup> group_dirty: atomic<u32>;

// dispatch (tile/8, tile/8, resident slots), after PreSim body collisions.
@compute @workgroup_size(8, 8)
fn classify_tiles(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    if lane == 0u { atomicStore(&group_dirty, 0u); }
    workgroupBarrier();
    let tile = schedule.counts.y;
    if id.z < schedule.counts.x && all(id.xy < vec2<u32>(tile)) {
        let i = id.z * tile * tile + id.y * tile + id.x;
        let geo = geometry[i];
        let rest = vec4<f32>(geo.x, 0.0, 0.0, 0.0);
        let pristine = all(state_a[i] == rest) && all(state_b[i] == rest)
            && history_rests(i, geo.x) && all(surface[i] == rest)
            && normal_rests(i)
            && all(geo.zw == vec2<f32>(0.0));
        if !pristine { atomicOr(&group_dirty, 1u); }
    }
    workgroupBarrier();
    if lane == 0u && atomicLoad(&group_dirty) != 0u && id.z < schedule.counts.x {
        atomicOr(&dirty[id.z], 1u);
    }
}

// dispatch (ceil(slots / 64), 1, 1). Worklist order is arbitrary; every cell
// reads only the source state, so it does not change any result.
@compute @workgroup_size(64)
fn build_worklist(@builtin(global_invocation_id) id: vec3<u32>) {
    let slot = id.x;
    if slot >= schedule.counts.x { return; }
    let t = slot_tiles[slot];
    let halo = i32(schedule.counts.z);
    var awake = schedule.table.z == 0u;
    for (var k = 0u; k < schedule.counts.w && !awake; k++) {
        let r = schedule.seeds[k];
        awake = all(t >= r.xy - vec2<i32>(halo)) && all(t <= r.zw + vec2<i32>(halo));
    }
    let size = vec2<i32>(schedule.table.xy);
    for (var y = -halo; y <= halo && !awake; y++) {
        for (var x = -halo; x <= halo && !awake; x++) {
            let n = t + vec2<i32>(x, y);
            if all(n >= vec2<i32>(0)) && all(n < size) {
                let s = tile_slots[u32(n.y) * schedule.table.x + u32(n.x)];
                if s >= 0 {
                    awake = atomicLoad(&dirty[u32(s)]) != 0u;
                }
            }
        }
    }
    if awake {
        worklist[atomicAdd(&indirect_args[2], 1u)] = slot;
    }
}
