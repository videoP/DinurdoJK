// Bounded Beer-Lambert medium, captured before water and diagnostic overlays.
struct Optics {
    inverse_view_projection: mat4x4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    fog: vec4<f32>,
    params: vec4<f32>,
    minimum: array<vec4<f32>, 8>,
    maximum: array<vec4<f32>, 8>,
};
COLOR_DECLARATION
@group(0) @binding(1) var scene_sampler: sampler;
DEPTH_DECLARATION
@group(0) @binding(3) var<uniform> optics: Optics;
// OCEAN_STRUCT
@group(1) @binding(1) var wave_normal: texture_2d_array<f32>;
@group(1) @binding(2) var wave_sampler: sampler;
@group(1) @binding(3) var<uniform> ocean: OceanRenderSettings;

@vertex fn vs_main(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}
fn interval(a: vec3<f32>, delta: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec2<f32> {
    var near = 0.0;
    var far = 1.0;
    for (var axis = 0u; axis < 3u; axis += 1u) {
        if (abs(delta[axis]) < 0.00001) {
            if (a[axis] < lo[axis] || a[axis] > hi[axis]) { return vec2<f32>(1.0, 0.0); }
        } else {
            let t0 = (lo[axis] - a[axis]) / delta[axis];
            let t1 = (hi[axis] - a[axis]) / delta[axis];
            near = max(near, min(t0, t1));
            far = min(far, max(t0, t1));
        }
    }
    return vec2<f32>(near, far);
}
fn slopes(p: vec2<f32>) -> vec2<f32> {
    let scale = ocean.map_scales[1];
    let uv = p / ocean.ocean_info.z * vec2<f32>(1.0, -1.0) * scale.xy;
    return textureSampleLevel(wave_normal, wave_sampler, uv, 1, 0.0).xy * vec2<f32>(1.0, -1.0) * scale.w;
}
struct Capture {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
};
fn capture_sample(uv: vec2<f32>, depth: f32, sample_color: vec3<f32>) -> Capture {
    let h = optics.inverse_view_projection * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), depth, 1.0);
    let world = h.xyz / h.w;
    let delta = world - optics.eye.xyz;
    let ray_length = length(delta);
    // World-position derivatives are also our cheapest reliable screen-space
    // discontinuity signal. At silhouettes they span unrelated surfaces, which
    // previously produced bogus normals and black caustic outlines. Keep these
    // derivatives outside conditional control flow so WGSL derivative-uniformity
    // requirements remain satisfied.
    let world_dx = dpdx(world);
    let world_dy = dpdy(world);
    let world_pixel_span = max(length(world_dx), length(world_dy));
    let viewport = vec2<f32>(textureDimensions(scene_color));
    let expected_pixel_span = max(ray_length * 4.0 / max(min(viewport.x, viewport.y), 1.0), 0.25);
    let caustic_surface_confidence = 1.0 - smoothstep(
        expected_pixel_span * 4.0,
        expected_pixel_span * 12.0,
        world_pixel_span,
    );
    let normal_cross = cross(world_dx, world_dy);
    let normal_length = length(normal_cross);
    var scene_normal = vec3<f32>(0.0, 1.0, 0.0);
    if (normal_length > 0.000001) { scene_normal = normal_cross / normal_length; }
    var color = sample_color;
    var spans: array<vec2<f32>, 8>;
    var count = 0u;
    var eye_depth = 0.0;
    var bottom_depth = 0.0;
    for (var i = 0u; i < min(u32(optics.params.w), 8u); i += 1u) {
        let lo = optics.minimum[i].xyz;
        let hi = optics.maximum[i].xyz;
        let span = interval(optics.eye.xyz, delta, lo, hi);
        if (span.y > span.x) {
            // Insertion sort allows exact union of overlapping bounds (no double fog).
            var j = count;
            loop {
                if (j == 0u) { break; }
                if (spans[j-1u].x <= span.x) { break; }
                spans[j] = spans[j-1u]; j -= 1u;
            }
            spans[j] = span; count += 1u;
            eye_depth = max(eye_depth, max(hi.y - (optics.eye.y + delta.y * span.x), 0.0));
        }
        if (all(world >= lo) && all(world <= hi)) { bottom_depth = max(bottom_depth, hi.y - world.y); }
    }
    var water_fraction = 0.0;
    var end = 0.0;
    for (var i = 0u; i < count; i += 1u) {
        water_fraction += max(spans[i].y - max(spans[i].x, end), 0.0);
        end = max(end, spans[i].y);
    }
    // Optional low-cost wave-curvature caustics. Approximation, not photon tracing.
    // Scope to the bound simulation; distinct authored oceans never borrow its waves.
    let own_water = ocean.authored_plane.x < 0.5 || (all(world.xz >= ocean.authored_bounds.xy) && all(world.xz <= ocean.authored_bounds.zw));
    let sun = normalize(-ocean.sun_direction_intensity.xyz);
    if (optics.params.z > 0.0 && bottom_depth > 2.0 && depth > 0.0 && own_water && sun.y > 0.05) {
        let refracted = refract(-sun, vec3<f32>(0.0, 1.0, 0.0), 1.0 / 1.333);
        let surface = world.xz - refracted.xz * bottom_depth / max(-refracted.y, 0.1);
        let step = 8.0;
        let dx = (slopes(surface + vec2<f32>(step,0.0)) - slopes(surface - vec2<f32>(step,0.0))) / (2.0*step);
        let dz = (slopes(surface + vec2<f32>(0.0,step)) - slopes(surface - vec2<f32>(0.0,step))) / (2.0*step);
        let focus = clamp(1.0 / max(0.25, 1.0 + bottom_depth * 0.25 * (dx.x + dz.y)) - 1.0, -0.4, 2.0);
        let fade = exp2(-4.321928 * bottom_depth / max(optics.fog.w, 1.0));
        let caustic = focus * optics.params.z * fade * abs(dot(scene_normal, sun)) * min(ocean.sun_direction_intensity.w, 1.0);
        // This is a screen-space curvature approximation, not a full light
        // transport solve. Never let its defocusing lobe drive scene radiance
        // through/below zero, and fade it out where neighboring pixels clearly
        // belong to different surfaces. The latter prevents player/prop/world
        // silhouettes from becoming artificial dark contour lines underwater.
        let caustic_multiplier = max(1.0 + caustic, 0.65);
        color *= mix(1.0, caustic_multiplier, caustic_surface_confidence);
    }
    if (water_fraction > 0.0) {
        // log2(20) corresponds to FIVE percent transmission, not four.
        let ratios = vec3<f32>(38.0/7.5, 38.0/22.0, 1.0);
        let transmission = exp2(-4.321928 * ratios * ray_length * water_fraction / max(optics.fog.w, 1.0));
        let downwelling = exp2(-4.321928 * ratios * eye_depth / max(optics.params.x, 1.0));
        color = color * transmission + optics.fog.rgb * downwelling * (1.0 - transmission);
    }
    var out: Capture;
    out.color = vec4<f32>(color, 1.0);
    out.depth = select(max(dot(delta, optics.forward.xyz), 0.0), 1000000.0, depth <= 0.0);
    return out;
}

@fragment fn fs_capture(@builtin(position) p: vec4<f32>) -> Capture {
    let pixel = vec2<i32>(p.xy);
    let uv = p.xy / vec2<f32>(textureDimensions(scene_color));
    CAPTURE_BODY
}
