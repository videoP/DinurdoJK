// JKA experiment: measure the strongest horizontal compression in each FFT
// band. A spatially constant budget avoids the extra folds caused by varying
// a displacement multiplier independently at neighbouring vertices.
@group(0) @binding(0) var derivatives: texture_2d_array<f32>;
@group(0) @binding(1) var extras: texture_2d_array<f32>;
@group(0) @binding(2) var<storage, read_write> partial: array<f32>;
@group(0) @binding(3) var<storage, read_write> limits: vec4<f32>;
struct Clock { time: vec4<f32> }
@group(0) @binding(4) var<uniform> clock: Clock;
var<workgroup> maximum: array<vec4<f32>, 64>;

@compute @workgroup_size(8, 8)
fn scan(@builtin(global_invocation_id) id: vec3<u32>,
        @builtin(local_invocation_index) lane: u32,
        @builtin(workgroup_id) group: vec3<u32>) {
    let size = textureDimensions(derivatives);
    var compression = 0.0;
    if all(id.xy < size) {
        let d = textureLoad(derivatives, vec2<i32>(id.xy), i32(id.z), 0);
        let cross_term = textureLoad(extras, vec2<i32>(id.xy), i32(id.z), 0).x;
        let least = 0.5 * (d.z + d.w - length(vec2<f32>(d.z - d.w, 2.0 * cross_term)));
        compression = max(-least, 0.0);
    }
    maximum[lane] = vec4<f32>(compression);
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride /= 2u) {
        if lane < stride { maximum[lane] = max(maximum[lane], maximum[lane + stride]); }
        workgroupBarrier();
    }
    if lane == 0u {
        let groups = (size + vec2<u32>(7u)) / 8u;
        partial[(group.z * groups.y + group.y) * groups.x + group.x] = maximum[0].x;
    }
}

@compute @workgroup_size(64)
fn finish(@builtin(local_invocation_index) lane: u32) {
    let size = textureDimensions(derivatives);
    let groups = (size + vec2<u32>(7u)) / 8u;
    let count = groups.x * groups.y;
    let layers = textureNumLayers(derivatives);
    var value = vec4<f32>(0.0);
    for (var i = lane; i < count; i += 64u) {
        for (var band = 0u; band < layers; band++) {
            value[band] = max(value[band], partial[band * count + i]);
        }
    }
    maximum[lane] = value;
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride /= 2u) {
        if lane < stride { maximum[lane] = max(maximum[lane], maximum[lane + stride]); }
        workgroupBarrier();
    }
    if lane == 0u {
        // Attack immediately; release slowly so a new peak cannot invert the
        // mesh and successive FFT frames do not visibly pump the chop.
        limits = vec4<f32>(max(maximum[0].xyz, limits.xyz * exp(-max(clock.time.z, 0.0) * 0.6)), limits.w);
    }
}
