// Read-only probe of the contact and Ninja resources used by this tick.
// Buffer layouts mirror GpuTrack and jka_ninja::RiverWakeDebug/HashParams.
struct Track {
    origin: vec4<f32>,
    previous: vec4<f32>,
    mins: vec4<f32>,
    maxs: vec4<f32>,
    velocity: vec4<f32>,
    previous_velocity: vec4<f32>,
    ids: vec4<u32>,
}
struct ContactControls { extent: vec4<f32>, count: vec4<u32> }
struct HashParams { dims: vec4<u32>, counts: vec4<u32>, factors: vec4<f32> }
struct Stroke {
    position: vec4<f32>,
    previous: vec4<f32>,
    velocity: vec4<f32>,
    radius: f32,
    admitted: u32,
    padding: vec2<u32>,
}
@group(0) @binding(0) var<uniform> native: ContactControls;
@group(0) @binding(1) var state: texture_2d_array<f32>;
@group(0) @binding(2) var<storage, read> tracks: array<Track>;
@group(0) @binding(3) var<storage, read> flags: array<u32>;
@group(0) @binding(4) var<uniform> selection: vec4<u32>; // first, count
@group(0) @binding(5) var<storage, read_write> result: array<vec4<f32>, 13>;
@group(0) @binding(6) var<uniform> hash: HashParams;
@group(0) @binding(7) var<storage, read> strokes: array<Stroke>;
@group(0) @binding(8) var<storage, read> global_list: array<u32>;
@group(0) @binding(9) var<storage, read> neighbors: array<u32>;
@group(0) @binding(10) var painter_before: texture_2d<f32>;
@group(0) @binding(11) var painter_after: texture_2d<f32>;
@group(0) @binding(12) var velocity_density: texture_2d<f32>;
@group(0) @binding(13) var pressure: texture_2d<f32>;

@compute @workgroup_size(1)
fn probe_contacts() {
    var contacts = 0u;
    for (var i = 0u; i < native.count.x; i++) {
        contacts += u32(flags[i] != 0u);
    }
    var index = selection.x;
    var subject_contacts = 0u;
    for (var i = selection.x; i < selection.x + selection.y; i++) {
        if flags[i] != 0u {
            if subject_contacts == 0u { index = i; }
            subject_contacts++;
        }
    }
    let track = tracks[index];
    let dims = textureDimensions(state, 0);
    let cell = native.extent.xy / vec2<f32>(dims);
    let at = vec2<i32>(floor((track.origin.xy + 0.5 * native.extent.xy) / cell));
    var reason = 0u; // out of XY, dry, water below/above band, within band
    var depth = 0.0;
    var height = 0.0;
    if all(at >= vec2<i32>(0)) && all(at < vec2<i32>(dims)) {
        depth = textureLoad(state, at, 1, 0).x;
        height = textureLoad(state, at, 0, 0).x;
        reason = 1u;
        if depth > 1e-5 {
            reason = 4u;
            if height < track.mins.z { reason = 2u; }
            if height > track.maxs.z { reason = 3u; }
        }
    }
    result[0] = vec4<f32>(f32(contacts), f32(subject_contacts), f32(index), f32(reason));
    result[1] = vec4<f32>(vec2<f32>(at), depth, height);
}

fn base_at(at: vec2<u32>) -> u32 {
    return (at.y * hash.dims.x + at.x) * (1u + hash.dims.w);
}

fn center(uv: vec2<f32>, dims: vec2<u32>) -> vec2<i32> {
    return vec2<i32>(clamp(floor(uv * vec2<f32>(dims)), vec2<f32>(0.0), vec2<f32>(dims - 1u)));
}

fn finite3(value: vec3<f32>) -> bool {
    return all((bitcast<vec3<u32>>(value) & vec3<u32>(0x7f800000u)) != vec3<u32>(0x7f800000u));
}

// Count the selected cell's candidates before its capacity limit. The
// stored counts/IDs below are read from HashGrid2, independently of this.
fn selected_cell_overflow(at: vec2<u32>, stored: u32) -> u32 {
    let u = (vec2<f32>(at) + 0.5) / vec2<f32>(hash.dims.xy);
    let factor = f32(max(hash.dims.x, hash.dims.y)) / hash.factors.x;
    var candidates = 0u;
    for (var i = 0u; i < stored; i++) {
        let s = strokes[global_list[i + 1u]];
        let ua = u - s.position.xy;
        let ba = s.previous.xy - s.position.xy;
        let denom = dot(ba, ba);
        var h = select(0.0, 1.0, dot(ua, ba) > 0.0);
        if denom != 0.0 { h = clamp(dot(ua, ba) / denom, 0.0, 1.0); }
        let d = length(ua - ba * h);
        let q = round((d - s.radius) * factor) / factor;
        candidates += u32(q < 2.0 * s.radius);
    }
    return candidates - min(candidates, hash.dims.w);
}

@compute @workgroup_size(1)
fn probe_solver() {
    let index = u32(result[0].z);
    let s = strokes[index];
    var admitted = 0u;
    var passed = 0u;
    for (var i = 0u; i < hash.counts.x; i++) {
        let valid = strokes[i].admitted != 0u;
        admitted += u32(valid);
        passed += u32(valid && (hash.counts.y == 0u || flags[i] != 0u));
    }
    let stored = min(global_list[0], hash.dims.z);
    result[2] = vec4<f32>(f32(hash.counts.x), f32(admitted), f32(passed), f32(stored));
    result[10].w = f32(global_list[0] - stored);
    var occupied = 0u;
    var full = 0u;
    for (var i = 0u; i < hash.dims.x * hash.dims.y; i++) {
        let count = neighbors[i * (1u + hash.dims.w)];
        occupied += u32(count != 0u);
        full += u32(count == hash.dims.w);
    }
    result[3] = vec4<f32>(vec2<f32>(hash.dims.xy), f32(occupied), f32(full));
    result[5] = s.position;
    result[6] = vec4<f32>(s.velocity.xyz, s.previous.z);
    result[4].z = s.radius;
    let uv = s.position.xy;
    if any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) { return; }
    let at = vec2<u32>(center(uv, hash.dims.xy));
    let base = base_at(at);
    let count = neighbors[base];
    var listed = false;
    for (var i = 0u; i < count; i++) {
        listed = listed || neighbors[base + 1u + i] == index;
    }
    result[4] = vec4<f32>(f32(count), f32(u32(listed)), s.radius, f32(selected_cell_overflow(at, stored)));

    // Compare both raw Painter ping-pong sides at the same 5x5 texel ROI.
    let dims = textureDimensions(painter_after, 0);
    let mid = center(uv, dims);
    var before = vec4<f32>(0.0);
    var after = vec4<f32>(0.0);
    var delta = vec4<f32>(0.0);
    var sum = vec3<f32>(0.0);
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let p = mid + vec2<i32>(x, y);
            if any(p < vec2<i32>(0)) || any(p >= vec2<i32>(dims)) { continue; }
            let a = textureLoad(painter_before, p, 0).xyz;
            let b = textureLoad(painter_after, p, 0).xyz;
            if !finite3(a) || !finite3(b) { delta.w += 1.0; continue; }
            before = vec4<f32>(max(before.xyz, abs(a)), before.w + f32(u32(a.z > 0.0)));
            after = vec4<f32>(max(after.xyz, abs(b)), after.w + f32(u32(b.z > 0.0)));
            delta = vec4<f32>(max(delta.xyz, abs(b - a)), delta.w);
            sum += abs(b);
        }
    }
    result[7] = before;
    result[8] = after;
    result[9] = delta;
    result[10] = vec4<f32>(sum, result[10].w);
    result[11] = textureLoad(velocity_density, center(uv, textureDimensions(velocity_density, 0)), 0);
    result[12] = textureLoad(pressure, center(uv, textureDimensions(pressure, 0)), 0);
}
