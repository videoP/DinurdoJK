// Acoustic analysis only: read the existing solved presentation fields, not
// the optical waves or view-dependent spray particles. World XY/Z in cm.
struct Body { origin: vec4<f32>, grid: vec4<f32> }
@group(0) @binding(0) var<uniform> body: Body;
@group(0) @binding(1) var water: texture_2d<f32>;
@group(0) @binding(7) var foam: texture_2d<f32>;
@group(1) @binding(0) var<uniform> area: vec4<f32>; // lower XY, bin cm, unused

@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32((i << 1u) & 2u) * 2.0 - 1.0, f32(i & 2u) * 2.0 - 1.0, 0.0, 1.0);
}
struct Metrics { @location(0) a: vec4<f32>, @location(1) b: vec4<f32> }
@fragment fn fs(@builtin(position) p: vec4<f32>) -> Metrics {
    let bin = vec2<f32>(floor(p.x), f32(u32(p.y) % 16u));
    let dim = vec2<i32>(textureDimensions(water));
    let dx = body.grid.xy / vec2<f32>(dim);
    let foam_dim = vec2<i32>(textureDimensions(foam));
    var count = 0.0;
    var sum = vec4<f32>(0.0); // z, speed m/s, turbulence, rotational shear
    var center = vec2<f32>(0.0);
    var fall = 0.0;
    var peak_fall = 0.0;
    let samples = i32(clamp(ceil(area.z / max(min(dx.x, dx.y), 1.0)), 6.0, 12.0));
    // Stratified world-space quadrature includes narrower streams than a single
    // sample at each emitter. The same XY is used after a window recenter.
    for (var y = 0; y < samples; y++) { for (var x = 0; x < samples; x++) {
        let offset = (vec2<f32>(f32(x), f32(y)) + 0.5) / f32(samples);
        let world = area.xy + (bin + offset) * area.z;
        let uv = (world - body.origin.xy) / body.grid.xy + 0.5;
        let at = vec2<i32>(floor(uv * vec2<f32>(dim)));
        if any(at < vec2<i32>(1)) || any(at >= dim - vec2<i32>(1)) { continue; }
        let a = textureLoad(water, at, 0);
        if !(a.y > 1.0) || abs(a.x) > 65500.0 || any(abs(a.zw) > vec2<f32>(20000.0)) { continue; }
        let speed = length(a.zw) * 0.01;
        let f = textureLoad(foam, clamp(vec2<i32>(uv * vec2<f32>(foam_dim)), vec2<i32>(0), foam_dim - vec2<i32>(1)), 0).r;
        var grad_x = vec2<f32>(0.0);
        var grad_y = vec2<f32>(0.0);
        var n_x = 0.0;
        var n_y = 0.0;
        var drop = 0.0;
        var jump = 0.0;
        let fr = speed / sqrt(9.81 * max(a.y * 0.01, 0.01));
        for (var k = 0; k < 4; k++) {
            let off = array<vec2<i32>,4>(vec2<i32>(1,0), vec2<i32>(-1,0), vec2<i32>(0,1), vec2<i32>(0,-1))[k];
            let b = textureLoad(water, at + off, 0);
            if !(b.y > 1.0) || abs(b.x) > 65500.0 || any(abs(b.zw) > vec2<f32>(20000.0)) { continue; }
            let dir = vec2<f32>(off);
            let delta = (b.zw - a.zw) * dir.x / max(dx.x, 1.0);
            if off.x != 0 { grad_x += delta; n_x += 1.0; }
            else { grad_y += (b.zw - a.zw) * dir.y / max(dx.y, 1.0); n_y += 1.0; }
            // A lower wet cell receiving faster water from an uphill neighbour.
            let incoming = max(0.0, dot(b.zw, -dir)) * 0.01;
            let h = max(0.0, (b.x - a.x) * 0.01);
            let slope = max(0.0, (b.x - a.x) / max(dot(abs(dir), dx), 1.0));
            drop = max(drop, smoothstep(0.18, 0.8, h) * smoothstep(0.3, 1.2, slope) * smoothstep(0.35, 1.8, incoming));
            let up_fr = incoming / sqrt(9.81 * max(b.y * 0.01, 0.01));
            jump = max(jump, smoothstep(0.9, 1.6, up_fr) * (1.0 - smoothstep(0.65, 1.05, fr)) * smoothstep(0.35, 1.8, incoming));
        }
        grad_x /= max(n_x, 1.0);
        grad_y /= max(n_y, 1.0);
        let curl = smoothstep(0.3, 4.0, abs(grad_x.y - grad_y.x));
        let strain = length(vec3<f32>(grad_x.x, grad_y.y, (grad_x.y + grad_y.x) * 0.7071));
        let agitation = max(max(smoothstep(0.4, 5.0, strain), curl * 0.65), clamp(f, 0.0, 1.0) * 0.7);
        count += 1.0;
        center += offset;
        sum += vec4<f32>(body.origin.z + a.x, speed, max(agitation, jump), max(curl, jump));
        fall += drop;
        peak_fall = max(peak_fall, drop);
    }}
    let denom = max(count, 1.0);
    // Preserve local strong drop signals even when the rest of a bin is calm.
    return Metrics(vec4<f32>(sum.xyz / denom, count / f32(samples*samples)),
        vec4<f32>(sum.w / denom, clamp(max(fall / denom * 3.0, peak_fall * 0.5), 0.0, 1.0), center / denom));
}
