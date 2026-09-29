// Full-resolution, temporally rate-limited sun shadow tracing.
//
// Every pixel gets a fresh traced ray exactly once every 4 frames, following a
// 2x2 ordered-dither schedule so the traced subset is spread evenly across the
// screen each frame (this replaces an earlier, removed spatial half-resolution
// cache that traded resolution for rate and never paid for itself: reconstructing
// a downsampled cache required its own depth prepass and a fixed per-fragment
// reconstruction cost on every full-resolution receiver, whether or not a light
// was nearby). On frames a pixel is not scheduled, its value is carried forward
// by reprojecting through the existing TAA motion-vector buffer and validating
// against the world position recorded the last time that pixel was traced;
// disocclusion (reprojection out of bounds, or a world-position mismatch beyond
// tolerance) forces an immediate fresh trace instead of waiting for the next
// scheduled frame.
@group(1) @binding(0) var<uniform> rt_shadow_settings: RtSunShadowSettings;
@group(1) @binding(1) var rt_full_depth: texture_2d<f32>;
@group(1) @binding(2) var rt_motion_vectors: texture_2d<f32>;
@group(1) @binding(3) var rt_shadow_prev: texture_2d<f32>;
@group(1) @binding(4) var rt_shadow_out: texture_storage_2d<rgba32float, write>;
@group(1) @binding(5) var<uniform> lighting_settings: LightingSettings;
@group(1) @binding(6) var<uniform> shadow_settings: ShadowSettings;
@group(0) @binding(0) var<uniform> camera: Camera;

fn rt_shadow_depth_at(pixel: vec2<i32>) -> f32 {
    let max_pixel = vec2<i32>(rt_shadow_settings.dimensions.xy) - vec2<i32>(1);
    return textureLoad(rt_full_depth, clamp(pixel, vec2<i32>(0), max_pixel), 0).x;
}

fn rt_shadow_point_at(pixel: vec2<i32>) -> vec3<f32> {
    let max_pixel = vec2<i32>(rt_shadow_settings.dimensions.xy) - vec2<i32>(1);
    let clamped = clamp(pixel, vec2<i32>(0), max_pixel);
    return rt_shadow_world_position(vec2<f32>(clamped) + vec2<f32>(0.5), rt_shadow_depth_at(clamped));
}

// Ordered so consecutive frames touch diagonally opposite cells first, which
// spreads visible error more evenly than a raster (0,0),(1,0),(0,1),(1,1) order.
const RT_SHADOW_DITHER: array<vec2<u32>, 4> = array<vec2<u32>, 4>(
    vec2<u32>(0u, 0u), vec2<u32>(1u, 1u), vec2<u32>(1u, 0u), vec2<u32>(0u, 1u));

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= rt_shadow_settings.dimensions.xy)) { return; }
    let pixel = vec2<i32>(id.xy);
    let depth = textureLoad(rt_full_depth, pixel, 0).x;
    if (!(depth > 0.0 && depth < 999999.0)) {
        // No receiver here (sky/far plane): nothing to shadow, nothing to cache.
        textureStore(rt_shadow_out, pixel, vec4<f32>(1.0, 1.0e9, 1.0e9, 1.0e9));
        return;
    }
    let position = rt_shadow_world_position(vec2<f32>(pixel) + vec2<f32>(0.5), depth);
    let left = position - rt_shadow_point_at(pixel - vec2<i32>(1, 0));
    let right = rt_shadow_point_at(pixel + vec2<i32>(1, 0)) - position;
    let up = position - rt_shadow_point_at(pixel - vec2<i32>(0, 1));
    let down = rt_shadow_point_at(pixel + vec2<i32>(0, 1)) - position;
    let dx = select(right, left, dot(left, left) < dot(right, right) && dot(left, left) > 1e-10);
    let dy = select(down, up, dot(up, up) < dot(down, down) && dot(up, up) > 1e-10);
    let cross_normal = cross(dx, dy);
    var normal: vec3<f32>;
    var need_trace = rt_shadow_settings.flags.y == 0u;
    if (dot(cross_normal, cross_normal) > 1e-12) {
        normal = cross_normal * inverseSqrt(dot(cross_normal, cross_normal));
        normal *= select(-1.0, 1.0, dot(normal, camera.camera_pos_time.xyz - position) >= 0.0);
    } else {
        // Degenerate neighborhood (silhouette edge, depth discontinuity): fall
        // back to a camera-facing normal and force a fresh trace rather than
        // trusting a history sample validated against a normal we don't have.
        normal = normalize(camera.camera_pos_time.xyz - position);
        need_trace = true;
    }

    let cell = vec2<u32>(id.xy) % vec2<u32>(2u);
    let phase = rt_shadow_settings.dimensions.z % 4u;
    need_trace = need_trace || all(cell == RT_SHADOW_DITHER[phase]);

    var visibility = 1.0;
    if (!need_trace) {
        let dims = vec2<f32>(rt_shadow_settings.dimensions.xy);
        let motion = textureLoad(rt_motion_vectors, pixel, 0).xy;
        let history_uv = (vec2<f32>(pixel) + vec2<f32>(0.5)) / dims - motion;
        if (any(history_uv < vec2<f32>(0.0)) || any(history_uv > vec2<f32>(1.0))) {
            need_trace = true;
        } else {
            let history_pixel = clamp(vec2<i32>(history_uv * dims), vec2<i32>(0), vec2<i32>(dims) - vec2<i32>(1));
            let history = textureLoad(rt_shadow_prev, history_pixel, 0);
            let tolerance = clamp(depth * 0.02, 0.05, 2.0);
            if (distance(history.yzw, position) > tolerance) {
                need_trace = true;
            } else {
                visibility = history.x;
            }
        }
    }
    if (need_trace) {
        var input: VertexOut;
        input.clip_position = vec4<f32>(vec2<f32>(pixel) + vec2<f32>(0.5), 0.0, 1.0);
        input.world_position = position;
        visibility = rt_uncached_sun_visibility(input, normal);
    }
    textureStore(rt_shadow_out, pixel, vec4<f32>(visibility, position));
}
