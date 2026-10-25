// Bitmap-font quads and lightbox panels for the main menu, using JKA's stock
// font atlases (fonts/aurabesh, fonts/ergoec). Premultiplied "over" output.
// The same shader draws the vertical lightbox into the neon emission target
// and the info text into the CRT's low-resolution source image.
struct View {
    size: vec4<f32>, // xy = target pixels, z = seconds
};
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var aurebesh: texture_2d<f32>;
@group(0) @binding(2) var latin: texture_2d<f32>;
@group(0) @binding(3) var atlas_sampler: sampler;

struct In {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // x = mode (0 solid, 1 Aurebesh glyph, 2 Latin glyph, 3 lightbox panel),
    // yz = panel size px, w = seed
    @location(3) params: vec4<f32>,
};
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>,
};

@vertex
fn vs_main(in: In) -> Out {
    var out: Out;
    out.position = vec4<f32>(in.position / view.size.xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.params = in.params;
    return out;
}

fn rounded_box(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let mode = u32(in.params.x + 0.5);
    if (mode == 1u) {
        let a = textureSample(aurebesh, atlas_sampler, in.uv).a * in.color.a;
        return vec4<f32>(in.color.rgb * a, a);
    }
    if (mode == 2u) {
        let a = textureSample(latin, atlas_sampler, in.uv).a * in.color.a;
        return vec4<f32>(in.color.rgb * a, a);
    }
    if (mode == 3u) {
        // A backlit acrylic sign: two fluorescent tubes behind a diffuser,
        // a hotter band where they sit, a dark aluminium frame.
        let size = in.params.yz;
        let p = (in.uv - 0.5) * size;
        let outer = rounded_box(p, size * 0.5, 5.0);
        let frame = 4.0;
        let inner = rounded_box(p, size * 0.5 - vec2<f32>(frame), 3.0);
        let coverage = 1.0 - smoothstep(-0.75, 0.75, outer);
        let face = 1.0 - smoothstep(-0.75, 0.75, inner);
        let x = in.uv.x;
        let tubes = 0.80 + 0.12 * exp(-pow((x - 0.3) / 0.16, 2.0)) + 0.12 * exp(-pow((x - 0.7) / 0.16, 2.0));
        let falloff = 1.0 - 0.10 * pow(abs(in.uv.y - 0.5) * 2.0, 3.0);
        let edge_shadow = smoothstep(0.0, 9.0, -inner);
        let lit = in.color.rgb * tubes * falloff * mix(0.82, 1.0, edge_shadow);
        let metal = vec3<f32>(0.018, 0.019, 0.022) * (1.0 + 0.6 * smoothstep(-frame, 0.0, -abs(inner + frame * 0.5)));
        let rgb = mix(metal, lit, face);
        return vec4<f32>(rgb * coverage, coverage);
    }
    return vec4<f32>(in.color.rgb * in.color.a, in.color.a);
}
