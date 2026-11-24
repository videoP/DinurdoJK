// DinurdoJK native input substitution, NOT the Fluid Ninja painter.
// Same hydraulic cell-centre / water height versus box test as
// river_collisions.wgsl::stamp_bodies. Do not query the post-stamp solid mask:
// the actor being tested may itself have written that collider.
struct Controls { extent: vec4<f32>, count: vec4<u32> }
struct Track {
    origin: vec4<f32>,  // relative to domain centre, cm; w = source kind
    previous: vec4<f32>, // w = valid history
    mins: vec4<f32>,
    maxs: vec4<f32>,
    velocity: vec4<f32>, // cm/s; w = native velocity available
    previous_velocity: vec4<f32>, // same contract, w also requires valid history
    ids: vec4<u32>, // id, continuity, time_ms bits, previous time_ms bits
}
@group(0) @binding(0) var<uniform> controls: Controls;
@group(0) @binding(1) var state: texture_2d_array<f32>;
@group(0) @binding(2) var<storage, read> tracks: array<Track>;
@group(0) @binding(3) var<storage, read_write> contact: array<atomic<u32>>;

@compute @workgroup_size(64)
fn contacts(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let actor = group.x;
    if actor >= controls.count.x { return; }
    let t = tracks[actor];
    let dims = textureDimensions(state, 0);
    let cell = controls.extent.xy / vec2<f32>(dims);
    var lo_f = ceil((t.mins.xy + 0.5 * controls.extent.xy) / cell - 0.5);
    var hi_f = floor((t.maxs.xy + 0.5 * controls.extent.xy) / cell - 0.5);
    // A tracked skeletal point is not a hull. Read its containing hydraulic
    // column, then test the reference tracking band against the wet surface.
    if t.mins.w != 0.0 {
        lo_f = floor((t.origin.xy + 0.5 * controls.extent.xy) / cell);
        hi_f = lo_f;
    }
    // Clamp before conversion: a remote actor can lie well beyond the window.
    if any(hi_f < vec2<f32>(0.0)) || any(lo_f >= vec2<f32>(dims)) { return; }
    let lo = vec2<u32>(max(lo_f, vec2<f32>(0.0)));
    let hi = vec2<u32>(min(hi_f, vec2<f32>(dims - vec2<u32>(1))));
    if any(lo > hi) { return; }
    let span = hi - lo + vec2<u32>(1);
    for (var index = lane; index < span.x * span.y; index += 64u) {
        let at = vec2<i32>(lo + vec2<u32>(index % span.x, index / span.x));
        let depth = textureLoad(state, at, 1, 0).x;
        let height = textureLoad(state, at, 0, 0).x;
        if depth > 1e-5 && height >= t.mins.z && height <= t.maxs.z {
            atomicOr(&contact[actor], 1u);
            return;
        }
    }
}
