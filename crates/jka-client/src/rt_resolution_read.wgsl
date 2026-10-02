struct RtSunShadowFlags {
    flags: vec4<u32>, // enabled, reserved, reserved, reserved
};

@group(3) @binding(15) var<uniform> rt_shadow_flags: RtSunShadowFlags;
@group(3) @binding(16) var rt_sun_shadow: texture_2d<f32>;

// The temporal trace pass (rt_resolution_trace.wgsl) has already resolved this
// frame's sun visibility for every full-resolution pixel, so the receiver side
// is a same-pixel lookup: no spatial neighbor search, no per-light state.
//
// The cache is keyed by screen pixel and holds the surface the depth prepass
// saw there. A receiver that is not that surface (ocean and other surfaces the
// prepass skips, MSAA silhouette samples, vertex-deformed geometry) would read
// the shadow of whatever is behind it, so each texel also carries the camera
// distance it was traced for; on a mismatch return -1 and let the caller trace
// this fragment directly.
fn rt_cached_sun_visibility(input: VertexOut) -> f32 {
    if (rt_shadow_flags.flags.x == 0u || camera.render_flags.x != 0u) { return -1.0; }
    let dims = vec2<i32>(textureDimensions(rt_sun_shadow));
    let pixel = clamp(vec2<i32>(input.clip_position.xy), vec2<i32>(0), dims - vec2<i32>(1));
    let cached = textureLoad(rt_sun_shadow, pixel, 0).xy;
    let expected = distance(input.world_position, camera.camera_pos_time.xyz);
    if (abs(cached.y - expected) > max(expected * 0.005, 0.5)) { return -1.0; }
    return cached.x;
}
