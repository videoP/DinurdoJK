// Native per-cell storage for shallow.wgsl: the dense rectangle and the sparse
// pool at r_riverStorageMode 0. PreSim bottom history, WaterInfo and FP32
// normals are real buffers, accessed exactly as the stage bodies did inline.
@group(0) @binding(4) var<storage, read_write> previous_bottom: array<f32>;
@group(0) @binding(6) var<storage, read_write> output_normals: array<vec4<f32>>;
@group(0) @binding(8) var<storage, read> water_info: array<vec4<f32>>; // local height, depth, flow XY

fn previous_bottom_at(i: u32) -> f32 { return previous_bottom[i]; }
fn store_previous_bottom(i: u32, bottom: f32) { previous_bottom[i] = bottom; }
fn water_info_at(i: u32) -> vec4<f32> { return water_info[i]; }
fn store_normal(i: u32, normal: vec3<f32>) { output_normals[i] = vec4<f32>(normal, 0.0); }
