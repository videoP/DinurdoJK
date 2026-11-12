// Sparse pool storage, r_riverStorageMode 1 (not Unreal; appended after
// shallow_sparse.wgsl). Two buffers are derived instead of stored:
// - PreSim bottom history. A committed bed never changes (moving-body stamps
//   rewrite only collider and collision velocity), so after the first solver
//   frame PrevBottomContour equals BottomContour. The first frame reads the
//   Niagara reset zero through tiles.counts.y, which the host clears once
//   that frame is encoded. Tiles committed later (TileInit) start at bed.
// - WaterInfo. Simulation-first rivers have a dry WaterInfo: always zero.
// The host never dispatches snapshot_bottom in this mode.
@group(0) @binding(6) var<storage, read_write> output_normals: array<vec4<f32>>;

fn previous_bottom_at(i: u32) -> f32 { return select(geometry[i].x, 0.0, tiles.counts.y != 0u); }
fn store_previous_bottom(i: u32, bottom: f32) {}
fn water_info_at(i: u32) -> vec4<f32> { return vec4<f32>(0.0); }
fn store_normal(i: u32, normal: vec3<f32>) { output_normals[i] = vec4<f32>(normal, 0.0); }
