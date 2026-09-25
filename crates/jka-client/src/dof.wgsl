@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var linear_depth_texture: texture_2d<f32>;

struct DofSettings {
    focus_strength: vec4<f32>, // focus distance, strength, width, height
    quality: vec4<f32>, // x: 0 performance, 1 adaptive, 2 high
};
@group(0) @binding(3) var<uniform> settings: DofSettings;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    let p = positions[vertex_index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

fn source(uv: vec2<f32>) -> vec3<f32> {
    return textureSample(
        scene_texture,
        scene_sampler,
        clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0))
    ).rgb;
}

fn depth_at_uv(uv: vec2<f32>) -> f32 {
    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let pixel = clamp(
        vec2<i32>(vec2<f32>(uv.x, uv.y) * dims_f),
        vec2<i32>(0),
        max_pixel
    );
    return textureLoad(linear_depth_texture, pixel, 0).r;
}

fn coc_radius_px(depth: f32) -> f32 {
    let strength = clamp(settings.focus_strength.y, 0.0, 1.0);
    if (strength <= 0.001 || depth <= 0.0 || depth >= 999999.0) {
        return 0.0;
    }

    // Same core quantity used by physical CoC models: defocus grows with the
    // relative displacement from the focal plane. We expose only a strength
    // control, so it maps to a bounded maximum CoC diameter instead of asking
    // players for aperture/focal-length camera parameters.
    let focus = max(settings.focus_strength.x, 32.0);
    let relative_defocus = abs(depth - focus) / max(depth, 32.0);
    let max_radius = mix(2.0, 14.0, strength);
    return smoothstep(0.015, 0.95, relative_defocus) * max_radius;
}

@fragment
fn fs_horizontal(input: VertexOut) -> @location(0) vec4<f32> {
    let center = source(input.uv);
    let radius = coc_radius_px(depth_at_uv(input.uv));
    if (radius < 0.20) {
        return vec4<f32>(center, 1.0);
    }

    // Bevy's fast DOF uses a separable Gaussian whose support follows CoC.
    // PERFORMANCE always uses the compact five-sample kernel. ADAPTIVE only
    // pays for the denser kernel once wide CoC would expose sample spacing;
    // HIGH uses it for every blurred pixel.
    let texel_x = 1.0 / max(settings.focus_strength.z, 1.0);
    let high_quality = settings.quality.x > 1.5 ||
        (settings.quality.x > 0.5 && radius >= 6.0);
    var blurred: vec3<f32>;
    if (high_quality) {
        let scale = radius / 7.30294072;
        let o1 = 1.45842952 * scale * texel_x;
        let o2 = 3.40398481 * scale * texel_x;
        let o3 = 5.35180578 * scale * texel_x;
        let o4 = 7.30294072 * scale * texel_x;
        blurred = center * 0.13357122;
        blurred += (source(input.uv + vec2<f32>( o1, 0.0)) + source(input.uv + vec2<f32>(-o1, 0.0))) * 0.23330843;
        blurred += (source(input.uv + vec2<f32>( o2, 0.0)) + source(input.uv + vec2<f32>(-o2, 0.0))) * 0.13592781;
        blurred += (source(input.uv + vec2<f32>( o3, 0.0)) + source(input.uv + vec2<f32>(-o3, 0.0))) * 0.05138318;
        blurred += (source(input.uv + vec2<f32>( o4, 0.0)) + source(input.uv + vec2<f32>(-o4, 0.0))) * 0.01259497;
    } else {
        let scale = radius / 3.23076923;
        let o1 = 1.38461538 * scale * texel_x;
        let o2 = 3.23076923 * scale * texel_x;
        blurred = center * 0.22702703;
        blurred += source(input.uv + vec2<f32>( o1, 0.0)) * 0.31621622;
        blurred += source(input.uv + vec2<f32>(-o1, 0.0)) * 0.31621622;
        blurred += source(input.uv + vec2<f32>( o2, 0.0)) * 0.07027027;
        blurred += source(input.uv + vec2<f32>(-o2, 0.0)) * 0.07027027;
    }
    let blend = smoothstep(0.20, 1.25, radius);
    return vec4<f32>(mix(center, blurred, blend), 1.0);
}
