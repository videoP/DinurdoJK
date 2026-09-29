struct RtSunShadowFlags {
    flags: vec4<u32>, // enabled, reserved, reserved, reserved
};

@group(3) @binding(15) var<uniform> rt_shadow_flags: RtSunShadowFlags;
@group(3) @binding(16) var rt_sun_shadow: texture_2d<f32>;

// The temporal trace pass (rt_resolution_trace.wgsl) has already resolved this
// frame's sun visibility for every full-resolution pixel, so the receiver side
// is just a same-pixel lookup: no spatial neighbor search, no per-light state.
fn rt_cached_sun_visibility(input: VertexOut) -> f32 {
    if (rt_shadow_flags.flags.x == 0u || camera.render_flags.x != 0u) { return -1.0; }
    let dims = vec2<i32>(textureDimensions(rt_sun_shadow));
    let pixel = clamp(vec2<i32>(input.clip_position.xy), vec2<i32>(0), dims - vec2<i32>(1));
    return textureLoad(rt_sun_shadow, pixel, 0).x;
}
