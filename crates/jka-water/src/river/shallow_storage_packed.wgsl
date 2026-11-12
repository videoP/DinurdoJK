// Sparse pool storage, r_riverStorageMode 2 (not Unreal): mode 1 plus
// ComputeNormals output packed to one octahedral 2x16 snorm word per cell
// (normal_codec.wgsl). Normals are presentation-only; no stage reads them.
// See shallow_storage_lean.wgsl for the derived history and WaterInfo.
@group(0) @binding(6) var<storage, read_write> output_normals: array<u32>;

fn previous_bottom_at(i: u32) -> f32 { return select(geometry[i].x, 0.0, tiles.counts.y != 0u); }
fn store_previous_bottom(i: u32, bottom: f32) {}
fn water_info_at(i: u32) -> vec4<f32> { return vec4<f32>(0.0); }
fn store_normal(i: u32, normal: vec3<f32>) { output_normals[i] = river_pack_normal(normal); }
