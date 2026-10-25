// Neon tubes for the main menu. Each instance is one glyph: a quad around its
// centre-line primitives (screen pixels, from neon_menu/font.rs). The fragment
// takes the distance to the nearest centre line and shades a glass tube around
// it: hot white core, saturated gas body, short self-glow. The long halo comes
// from the blur chain afterwards, so this pass stays tight around the tubes.
struct View {
    size: vec4<f32>, // xy = target pixels
    time: vec4<f32>, // x = seconds
};
struct ItemLight {
    // rgb = gas colour, w = how white the unlit phosphor coating looks.
    color: vec4<f32>,
    // x = lit level (flicker applied), y = fill front x (px, lit to its left),
    // z = tube radius px, w = press flash
    state: vec4<f32>,
    // x = faulty glyph index or -1, y = that glyph's level, z = seed, w = selected
    fault: vec4<f32>,
};
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> prims: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> items: array<ItemLight>;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) pixel: vec2<f32>,
    @location(1) @interpolate(flat) range: vec4<u32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @location(0) rect: vec4<f32>, @location(1) range: vec4<u32>) -> Out {
    let corners = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    let pixel = mix(rect.xy, rect.zw, corners[vertex]);
    var out: Out;
    out.position = vec4<f32>(pixel / view.size.xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.pixel = pixel;
    out.range = range;
    return out;
}

const TAU: f32 = 6.28318530718;

fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - (a + ab * t));
}

// `sweep` is positive (normalised on the CPU after the y flip).
fn arc_distance(p: vec2<f32>, c: vec2<f32>, r: f32, start: f32, sweep: f32) -> f32 {
    let q = p - c;
    var angle = atan2(q.y, q.x) - start;
    angle = angle - floor(angle / TAU) * TAU;
    if (angle <= sweep) {
        return abs(length(q) - r);
    }
    let e0 = c + r * vec2<f32>(cos(start), sin(start));
    let e1 = c + r * vec2<f32>(cos(start + sweep), sin(start + sweep));
    return min(distance(p, e0), distance(p, e1));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let p = in.pixel;
    var d = 1e6;
    for (var i = 0u; i < in.range.y; i += 1u) {
        let a = prims[(in.range.x + i) * 2u];
        let b = prims[(in.range.x + i) * 2u + 1u];
        if (a.x < 0.5) {
            d = min(d, segment_distance(p, a.yz, vec2<f32>(a.w, b.x)));
        } else {
            d = min(d, arc_distance(p, a.yz, a.w, b.x, b.y));
        }
    }
    let item = items[in.range.z];
    let r = max(item.state.z, 0.75);
    // Lit to the left of the power-on front, with a soft edge. Written as a
    // clamp, not a reversed smoothstep: a far-away front must not collapse
    // the edge width to zero in f32.
    var lit = item.state.x * clamp((item.state.y - p.x) / (r * 6.0) + 0.5, 0.0, 1.0);
    if (i32(in.range.w) == i32(item.fault.x)) {
        lit *= item.fault.y;
    }
    let gas = item.color.rgb;
    // Cross-section: the tube body, a narrow white-hot core and the bright
    // halo of gas glowing inside the glass, then the first metre of air glow.
    let body = 1.0 - smoothstep(r * 0.8, r * 1.2, d);
    let core = exp(-pow(d / (0.38 * r), 2.0));
    // A hard-driven tube lights more of the air around it.
    let near = exp(-d / (r * (1.0 + 0.45 * min(lit, 2.5))));
    let flash = 1.0 + item.state.w * 1.5;
    var light = gas * (body * 1.05 + near * 0.6) * lit * 1.5 * flash;
    light += mix(gas, vec3<f32>(1.0), 0.72) * core * lit * 1.2 * flash;
    // Unlit or dim tube: phosphor-coated glass catching ambient street light,
    // with a thin specular line along the upper side of the cylinder.
    let rim = exp(-pow((d - r * 0.45) / (0.18 * r), 2.0));
    let glass = mix(gas, vec3<f32>(1.0), item.color.w) * (body * 0.035 + rim * 0.04) * (1.0 - min(lit, 1.0));
    return vec4<f32>(light + glass, 0.0);
}
