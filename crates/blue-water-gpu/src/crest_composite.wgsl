// Sample the actual native, stochastically tiled surface. Unlike the strict
// sum of band maxima, this retains chop when the bands' peaks do not coincide.
// It is an artistic bound, not a proof over the infinite continuous ocean.
#import naval_ocean::waves::{displacement, waves}
@group(0) @binding(0) var<storage, read_write> partial: array<f32>;
@group(0) @binding(1) var<storage, read_write> limits: vec4<f32>;
@group(0) @binding(2) var<uniform> elapsed: vec4<f32>;
var<workgroup> peak: array<f32, 64>;

@compute @workgroup_size(8, 8)
fn scan(@builtin(global_invocation_id) id: vec3<u32>,
        @builtin(local_invocation_index) lane: u32,
        @builtin(workgroup_id) group: vec3<u32>) {
    // Two overlapping regions, at different scales, include both broad sets
    // and fine chop. A fixed lattice avoids camera-dependent shape changes.
    let width = select(waves.tile.x, waves.tile.y * 2.0, id.z == 1u);
    let point = (vec2<f32>(id.xy) + vec2<f32>(0.19, 0.61)) * (width / 128.0) - vec2<f32>(width * 0.5);
    let h = max(min(waves.tile.z / waves.info.z, 0.5), 0.05);
    let dx = (displacement(point + vec2<f32>(h, 0.0), h) - displacement(point - vec2<f32>(h, 0.0), h)) / (2.0 * h);
    let dz = (displacement(point + vec2<f32>(0.0, h), h) - displacement(point - vec2<f32>(0.0, h), h)) / (2.0 * h);
    // The symmetric part bounds directional compression even when changing
    // stochastic blend weights add a nonsymmetric displacement derivative.
    let least = 0.5 * (dx.x + dz.z - length(vec2<f32>(dx.x - dz.z, dx.z + dz.x)));
    peak[lane] = max(-least, 0.0);
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride /= 2u) {
        if lane < stride { peak[lane] = max(peak[lane], peak[lane + stride]); }
        workgroupBarrier();
    }
    if lane == 0u { partial[(group.z * 16u + group.y) * 16u + group.x] = peak[0]; }
}

@compute @workgroup_size(64)
fn finish(@builtin(local_invocation_index) lane: u32) {
    var value = 0.0;
    for (var i = lane; i < 512u; i += 64u) { value = max(value, partial[i]); }
    peak[lane] = value;
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride /= 2u) {
        if lane < stride { peak[lane] = max(peak[lane], peak[lane + stride]); }
        workgroupBarrier();
    }
    if lane == 0u {
        limits.w = max(peak[0] * 1.05, limits.w * exp(-max(elapsed.z, 0.0) * 0.6));
    }
}
