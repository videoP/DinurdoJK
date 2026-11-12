// Sparse river tile pool: per-tile bed commit, the camera presentation window
// gather and the host CFS inlet. Not Unreal systems; see
// jka_water::river::sparse. Pool index = slot * tile^2 + local_y * tile + local_x.
// History (binding 3), commit normals (7) and gathered pool normals (12) come
// from the r_riverStorageMode prelude (river_storage.rs).

fn pool_index(slot: u32, tile: u32, lane: vec2<u32>) -> u32 {
    return slot * tile * tile + lane.y * tile + lane.x;
}

// ---- Bed commit: capture.wgsl stages 5/6 for one tile, then TileInit ----
// The tile's square capture uses the river-wide cameras. Collision capture
// has no dilation. Its WaterInfo height is dry (zero), exactly as the dense
// patch, or with r_riverCaptureMode 1 the per-cell reachable-floor reference
// (river_reach.rs). The border collider follows the virtual rectangle.
struct Commit {
    cells: vec4<i32>,  // first cell xy, lattice cells xy
    slot: vec4<u32>,   // slot, tile size, capture resolution, Niagara reset (1) or TileInit (0)
    contour_floor: vec4<f32>,  // contour floor (cm), void threshold (cm), bed reference bound (1) or not (0), unused
    cameras: array<vec4<f32>, 5>, // local camera Z, +/- depth direction
}
@group(0) @binding(0) var<uniform> commit: Commit;
@group(0) @binding(1) var depths: texture_2d_array<f32>;
@group(0) @binding(2) var<storage, read_write> commit_geometry: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> commit_state_a: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> commit_state_b: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> commit_surface: array<vec4<f32>>;
// Stage 6 WaterInfo height per tile cell (row-major, local cm), mode 1 only.
@group(0) @binding(23) var<storage, read> commit_reference: array<f32>;

fn capture_depth(cell: vec2<i32>, layer: i32) -> f32 {
    let last = i32(commit.slot.z) - 1;
    return textureLoad(depths, clamp(cell, vec2<i32>(0), vec2<i32>(last)), layer, 0).x;
}
fn capture_bilinear(uv: vec2<f32>, layer: i32) -> f32 {
    let texel = uv * f32(commit.slot.z) - 0.5;
    let lower = vec2<i32>(floor(texel));
    let f = fract(texel);
    let a = mix(capture_depth(lower, layer), capture_depth(lower + vec2<i32>(1, 0), layer), f.x);
    let b = mix(capture_depth(lower + vec2<i32>(0, 1), layer), capture_depth(lower + vec2<i32>(1, 1), layer), f.x);
    return mix(a, b, f.y);
}

@compute @workgroup_size(8, 8)
fn commit_tile(@builtin(global_invocation_id) id: vec3<u32>) {
    let tile = commit.slot.y;
    if any(id.xy >= vec2<u32>(tile)) { return; }
    let floor_value = commit.contour_floor.x;
    // project_contours with a square tile capture: uv = cell centre.
    let uv = (vec2<f32>(id.xy) + 0.5) / f32(tile);
    var values: array<f32, 5>;
    for (var layer = 0; layer < 5; layer++) {
        values[layer] = floor_value;
        if all(uv >= vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0)) {
            values[layer] = max(commit.cameras[layer].x + commit.cameras[layer].y * capture_bilinear(uv, layer), floor_value);
        }
    }
    // prepare_geometry with dilation radius 0 and dry WaterInfo height 0,
    // or the mode 1 reachable-floor reference.
    var reference = 0.0;
    if commit.contour_floor.z != 0.0 {
        reference = commit_reference[id.y * tile + id.x];
    }
    var top = max(-100000.0, values[3]);
    var under = max(-100000.0, values[4]);
    top = max(top, values[1]);
    under = min(under, values[2]);
    if reference - under <= 0.0 && under > values[0] && top > under && top > values[0] {
        top = values[0];
    }
    let at = commit.cells.xy + vec2<i32>(id.xy);
    let upper = commit.cells.zw - 1;
    var border = any(at < vec2<i32>(2)) || any(at > upper - 2);
    var bottom = max(top, values[0]);
    // Void: nothing at all was captured in this column (every bed candidate
    // lies above the contour floor). It is a wall as tall as the capture.
    // The threshold sits half the capture headroom above the floor: the
    // empty-pixel reconstruction does not round exactly onto the floor.
    if bottom <= commit.contour_floor.y {
        border = true;
        bottom = commit.cameras[0].x;
    }
    let i = pool_index(commit.slot.x, tile, id.xy);
    commit_geometry[i] = vec4<f32>(bottom, select(0.0, 1.0, border), 0.0, 0.0);
    if commit.slot.w != 0u {
        // A simulation's first tiles: Niagara reset zero grids and history.
        commit_store_history(i, 0.0);
        commit_state_a[i] = vec4<f32>(0.0);
        commit_state_b[i] = vec4<f32>(0.0);
    } else {
        // Later tiles: a dense cell that stayed dry (sparse::TileInit).
        commit_store_history(i, bottom);
        commit_state_a[i] = vec4<f32>(bottom, 0.0, 0.0, 0.0);
        commit_state_b[i] = vec4<f32>(bottom, 0.0, 0.0, 0.0);
    }
    commit_surface[i] = vec4<f32>(0.0);
    commit_store_rest_normal(i);
}

// ---- Void walls for a dense prepared bed (same rule as commit_tile) ----
struct VoidWalls {
    counts: vec4<u32>,  // cells, unused x3
    heights: vec4<f32>, // void threshold (contour floor + margin), wall height, unused x2
}
@group(0) @binding(21) var<uniform> void_walls: VoidWalls;
@group(0) @binding(22) var<storage, read_write> void_geometry: array<vec4<f32>>;

@compute @workgroup_size(64)
fn wall_void(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x + id.y * 65535u * 64u;
    if i >= void_walls.counts.x { return; }
    if void_geometry[i].x <= void_walls.heights.x {
        void_geometry[i] = vec4<f32>(void_walls.heights.y, 1.0, 0.0, 0.0);
    }
}

// ---- Presentation window: dense copy of resident cells around the camera ----
// Non-resident cells read as dry, still water; render heights are rebased by
// the window's height offset to keep RGBA16f precision on long descents.
struct Window {
    origin: vec4<i32>,  // first cell xy, window cells xy
    table: vec4<u32>,  // table width, table height (tiles), tile, log2 tile
    lattice: vec4<i32>, // lattice cells xy, unused x2
    height: vec4<f32>,  // render height offset (cm), unused x3
}
@group(0) @binding(8) var<uniform> window: Window;
@group(0) @binding(9) var<storage, read> window_slots: array<i32>;
@group(0) @binding(10) var<storage, read> window_state: array<vec4<f32>>;
@group(0) @binding(11) var<storage, read> window_pool_surface: array<vec4<f32>>;
@group(0) @binding(13) var window_out_state: texture_storage_2d_array<r32float, write>;
@group(0) @binding(14) var<storage, read_write> window_out_surface: array<vec4<f32>>;
@group(0) @binding(15) var<storage, read_write> window_out_normals: array<vec4<f32>>;

@compute @workgroup_size(8, 8)
fn gather_window(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= vec2<u32>(window.origin.zw)) { return; }
    let at = window.origin.xy + vec2<i32>(id.xy);
    var water = vec4<f32>(0.0);
    var surface = vec4<f32>(0.0);
    var normal = vec4<f32>(0.0, 0.0, 1.0, 0.0);
    if all(at >= vec2<i32>(0)) && all(at < window.lattice.xy) {
        let tile = vec2<u32>(at) >> vec2<u32>(window.table.w);
        let slot = window_slots[tile.y * window.table.x + tile.x];
        if slot >= 0 {
            let lane = vec2<u32>(at) & vec2<u32>(window.table.z - 1u);
            let i = pool_index(u32(slot), window.table.z, lane);
            water = window_state[i];
            surface = window_pool_surface[i];
            normal = window_pool_normal(i);
        }
    }
    water.x -= window.height.x;
    surface.x -= window.height.x;
    let cell = vec2<i32>(id.xy);
    textureStore(window_out_state, cell, 0, vec4<f32>(water.x));
    textureStore(window_out_state, cell, 1, vec4<f32>(water.y));
    textureStore(window_out_state, cell, 2, vec4<f32>(water.z));
    textureStore(window_out_state, cell, 3, vec4<f32>(water.w));
    let o = id.y * u32(window.origin.z) + id.x;
    window_out_surface[o] = surface;
    window_out_normals[o] = normal;
}

// ---- Host CFS inlet (river_discharge.wgsl) on the pool, in place ----
// Q*dt over the open resident cells of the inlet rectangle whose bed is below
// the inlet level. Runs between source/sink and distribute overlap, as the
// dense adapter's ping-pong pass does; updating in place is equivalent.
struct Inlet {
    rect: vec4<i32>,   // first cell xy, exclusive end xy (lattice cells)
    table: vec4<u32>, // table width, table height (tiles), tile, log2 tile
    metrics: vec4<f32>, // cell area cm2, inlet level (local cm), volume cm3, unused
}
@group(0) @binding(16) var<uniform> inlet: Inlet;
@group(0) @binding(17) var<storage, read> inlet_slots: array<i32>;
@group(0) @binding(18) var<storage, read> inlet_geometry: array<vec4<f32>>;
@group(0) @binding(19) var<storage, read_write> inlet_count: atomic<u32>;
@group(0) @binding(20) var<storage, read_write> inlet_state: array<vec4<f32>>;

// Pool index of an eligible inlet cell, or -1.
fn inlet_cell(id: vec2<u32>) -> i32 {
    let at = inlet.rect.xy + vec2<i32>(id);
    if any(at >= inlet.rect.zw) || any(at < vec2<i32>(0)) { return -1; }
    let tile = vec2<u32>(at) >> vec2<u32>(inlet.table.w);
    let slot = inlet_slots[tile.y * inlet.table.x + tile.x];
    if slot < 0 { return -1; }
    let lane = vec2<u32>(at) & vec2<u32>(inlet.table.z - 1u);
    let i = pool_index(u32(slot), inlet.table.z, lane);
    let bed = inlet_geometry[i];
    if bed.y < 1.0 - 1e-5 && bed.x < inlet.metrics.y { return i32(i); }
    return -1;
}

@compute @workgroup_size(8, 8)
fn count_inlet(@builtin(global_invocation_id) id: vec3<u32>) {
    if inlet_cell(id.xy) >= 0 { atomicAdd(&inlet_count, 1u); }
}

@compute @workgroup_size(8, 8)
fn inject_inlet(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = inlet_cell(id.xy);
    let cells = atomicLoad(&inlet_count);
    if i < 0 || cells == 0u { return; }
    var water = inlet_state[u32(i)];
    water.y += inlet.metrics.z / (f32(cells) * inlet.metrics.x);
    water.x = inlet_geometry[u32(i)].x + water.y;
    inlet_state[u32(i)] = water;
}
