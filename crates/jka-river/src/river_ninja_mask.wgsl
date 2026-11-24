// DinurdoJK input replacement, not a Fluid Ninja simulation kernel.
// Native geometry addressing: shallow_dense.wgsl / river_tiles.wgsl gather.
// Native state height/depth/vx/vy layers: foam_field_stage.wgsl.
struct MaskControls {
    dims: vec4<u32>,  // xy window cells
    window_lattice: vec4<i32>, // xy window origin, zw virtual lattice cells
    table: vec4<u32>, // xy tile table dimensions, z tile size, w log2 tile size
    unused: vec4<u32>,
}
@group(0) @binding(0) var<uniform> controls: MaskControls;
@group(0) @binding(1) var water: texture_2d_array<f32>;
@group(0) @binding(2) var<storage, read> geometry: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> slots: array<i32>;
@group(0) @binding(4) var output_mask: texture_storage_2d<r32float, write>;

fn blocked(cell: vec2<i32>, index: u32) -> f32 {
    // Match the native wet/dry test. Shallow water is open to Fluid Ninja.
    if index >= arrayLength(&geometry) { return 1.0; }
    let depth = textureLoad(water, cell, 1, 0).x;
    let solid = geometry[index].y >= 1.0 - 1e-5;
    return select(1.0, 0.0, !solid && depth > 1e-5);
}
@compute @workgroup_size(8, 8)
fn dense_mask(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= controls.dims.xy) { return; }
    let index = id.y * controls.dims.x + id.x;
    textureStore(output_mask, id.xy, vec4<f32>(blocked(vec2<i32>(id.xy), index)));
}
@compute @workgroup_size(8, 8)
fn sparse_mask(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= controls.dims.xy) { return; }
    let at = controls.window_lattice.xy + vec2<i32>(id.xy);
    var value = 1.0;
    if all(at >= vec2<i32>(0)) && all(at < controls.window_lattice.zw) {
        let tile = vec2<u32>(at) >> vec2<u32>(controls.table.w);
        let table_index = tile.y * controls.table.x + tile.x;
        if table_index < arrayLength(&slots) {
            let slot = slots[table_index];
            if slot >= 0 {
                let lane = vec2<u32>(at) & vec2<u32>(controls.table.z - 1u);
                let index = u32(slot) * controls.table.z * controls.table.z + lane.y * controls.table.z + lane.x;
                value = blocked(vec2<i32>(id.xy), index);
            }
        }
    }
    textureStore(output_mask, id.xy, vec4<f32>(value));
}
