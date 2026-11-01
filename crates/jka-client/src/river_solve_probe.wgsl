// Infrequent steady-flow check, not a solver stage. Each workgroup summarizes
// an 8x8 tile of the actual hydraulic state against its preceding snapshot.
// The CPU reads back only one vec4 per tile, asynchronously.
@group(0) @binding(0) var current_state: texture_2d_array<f32>;
@group(0) @binding(1) var prior_state: texture_2d_array<f32>;
@group(0) @binding(2) var<storage, read_write> tile_results: array<vec4<f32>>;

var<workgroup> contributions: array<vec4<f32>, 64>;

@compute @workgroup_size(8, 8)
fn main(
    @builtin(global_invocation_id) pixel: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
    @builtin(workgroup_id) group: vec3<u32>,
) {
    let dimensions = textureDimensions(current_state);
    var sample = vec4<f32>(0.0);
    if all(pixel.xy < dimensions) {
        let xy = vec2<i32>(pixel.xy);
        let depth = max(textureLoad(current_state, xy, 1, 0).x, 0.0);
        let before_depth = max(textureLoad(prior_state, xy, 1, 0).x, 0.0);
        let wet = depth > 0.1 || before_depth > 0.1;
        var velocity_change = 0.0;
        if wet {
            let current_vx = textureLoad(current_state, xy, 2, 0).x;
            let current_vy = textureLoad(current_state, xy, 3, 0).x;
            let previous_vx = textureLoad(prior_state, xy, 2, 0).x;
            let previous_vy = textureLoad(prior_state, xy, 3, 0).x;
            velocity_change = length(vec2<f32>(current_vx - previous_vx, current_vy - previous_vy));
        }
        // Absolute depth change, velocity change, wet cell count, water volume.
        sample = vec4<f32>(abs(depth - before_depth), velocity_change, select(0.0, 1.0, wet), depth);
    }
    contributions[local_index] = sample;
    workgroupBarrier();
    if local_index == 0u {
        var result = vec4<f32>(0.0);
        for (var i = 0u; i < 64u; i++) {
            result += contributions[i];
        }
        let groups_wide = (dimensions.x + 7u) / 8u;
        tile_results[group.y * groups_wide + group.x] = result;
    }
}
