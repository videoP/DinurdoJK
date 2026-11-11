// Sparse tile-pool addressing for shallow.wgsl. Not an Unreal system: UE
// 5.8.1 simulates one dense rectangle. The stages still see a virtual dense
// rectangle (params.dimensions) with its zero-outside and clamp policies;
// only square tiles of it are resident. See jka_water::river::sparse.
// Storage index = slot * tile^2 + local_y * tile + local_x for every per-cell
// buffer (state, geometry, history, WaterInfo, surface and normals).
struct Tiles {
    table: vec4<u32>, // table width, table height (tiles), tile size, log2 tile size
    counts: vec4<u32>, // dispatched slots, unused, unused, unused
}
@group(0) @binding(1) var<storage, read> water: array<vec4<f32>>; // height, depth, vx, vy
@group(0) @binding(3) var<storage, read_write> output_water: array<vec4<f32>>;
@group(0) @binding(12) var<uniform> tiles: Tiles;
@group(0) @binding(13) var<storage, read> tile_slots: array<i32>; // -1: not resident
@group(0) @binding(14) var<storage, read> slot_tiles: array<vec2<i32>>;
@group(0) @binding(15) var<storage, read_write> occupancy: array<atomic<u32>>;

// One workgroup layer per resident slot: dispatch (tile/8, tile/8, slots).
fn invocation_cell(id: vec3<u32>) -> vec2<i32> {
    let slot = min(id.z, max(tiles.counts.x, 1u) - 1u);
    return slot_tiles[slot] * i32(tiles.table.z) + vec2<i32>(id.xy);
}
fn invocation_active(id: vec3<u32>, at: vec2<i32>) -> bool {
    return id.z < tiles.counts.x && all(id.xy < vec2<u32>(tiles.table.z)) && in_bounds(at);
}

fn in_bounds(at: vec2<i32>) -> bool {
    return all(at >= vec2<i32>(0)) && all(at < vec2<i32>(params.dimensions.xy));
}
// Valid only for in-bounds cells.
fn slot_of(at: vec2<i32>) -> i32 {
    let tile = vec2<u32>(at) >> vec2<u32>(tiles.table.w);
    return tile_slots[tile.y * tiles.table.x + tile.x];
}
fn local_index(at: vec2<i32>) -> u32 {
    let lane = vec2<u32>(at) & vec2<u32>(tiles.table.z - 1u);
    return lane.y * tiles.table.z + lane.x;
}
fn slot_index(slot: i32, at: vec2<i32>) -> u32 {
    return u32(slot) * tiles.table.z * tiles.table.z + local_index(at);
}
// Storage index of a resident cell (a stage's own cell).
fn index(at: vec2<i32>) -> u32 { return slot_index(slot_of(at), at); }
fn read_water(at: vec2<i32>) -> vec4<f32> {
    if !in_bounds(at) { return vec4<f32>(0.0); }
    let slot = slot_of(at);
    if slot < 0 { return vec4<f32>(0.0); }
    return water[slot_index(slot, at)];
}
fn write_water(at: vec2<i32>, value: vec4<f32>) {
    output_water[index(at)] = value;
}
// Outside the rectangle: the dense zero. Inside but not resident: the native
// border collider, so an unexpanded edge reflects water instead of draining it.
fn read_geometry(at: vec2<i32>) -> vec4<f32> {
    if !in_bounds(at) { return vec4<f32>(0.0); }
    let slot = slot_of(at);
    if slot < 0 { return vec4<f32>(0.0, 1.0, 0.0, 0.0); }
    return geometry[slot_index(slot, at)];
}

fn clamp_cell(at: vec2<i32>) -> vec2<i32> {
    return clamp(at, vec2<i32>(0), vec2<i32>(params.dimensions.xy) - 1);
}

// Hardware filtering cannot span pool slots. This is SF_Bilinear/AM_Clamp
// on the virtual rectangle with the CPU reference's arithmetic; it is only
// reached from sample_cubic's edge or overshoot fallback.
fn bilinear(unit: vec2<f32>, channel: u32) -> f32 {
    let size = vec2<f32>(params.dimensions.xy);
    let at = clamp(unit * size - 0.5, vec2<f32>(0.0), size - 1.0);
    let cell = vec2<i32>(floor(at));
    let f = at - vec2<f32>(cell);
    let upper = clamp_cell(cell + vec2<i32>(1));
    let a = read_water(cell)[channel];
    let b = read_water(vec2<i32>(upper.x, cell.y))[channel];
    let c = read_water(vec2<i32>(cell.x, upper.y))[channel];
    let d = read_water(upper)[channel];
    let bottom = a + f.x * (b - a);
    let top = c + f.x * (d - c);
    return bottom + f.y * (top - bottom);
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
