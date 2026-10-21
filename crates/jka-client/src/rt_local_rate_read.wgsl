// Binary scalar visibility for all 32 clustered lights and all four samples.
// Layer 0 holds packed surface data, the light-list signature, emitter seed,
// and sample-zero visibility. An optional second layer holds samples 1..3.
@group(3) @binding(17) var rt_local_rate_cache: texture_2d_array<u32>;

fn rt_rate_metadata(input: VertexOut) -> vec4<u32> {
    if (rt_shadow_flags.flags.y == 0u || camera.render_flags.x != 0u) { return vec4<u32>(0u); }
    let dims = vec2<i32>(textureDimensions(rt_local_rate_cache));
    let pixel = vec2<i32>(input.clip_position.xy);
    if (any(pixel < vec2<i32>(0)) || any(pixel >= dims)) { return vec4<u32>(0u); }
    let record = textureLoad(rt_local_rate_cache, pixel, 0, 0);
    let depth = distance(input.world_position, camera.camera_pos_time.xyz);
    if (record.x == 0u || abs(rt_rate_depth(record.x) - depth) > max(depth * 0.001, 0.05)) {
        return vec4<u32>(0u);
    }
    let slice = min(u32(clamp(log(max(depth, 1.0)) / log(65536.0), 0.0, 0.999999) * 24.0), 23u);
    let cached_slice = min(u32(clamp(log(max(rt_rate_depth(record.x), 1.0)) / log(65536.0), 0.0, 0.999999) * 24.0), 23u);
    if (slice != cached_slice) { return vec4<u32>(0u); }
    return record;
}

fn rt_rate_emitter_seed(input: VertexOut) -> u32 {
    let record = rt_rate_metadata(input);
    if (record.x != 0u) { return record.z; }
    return rt_emitter_seed(input);
}

fn rt_rate_local_visibility(input: VertexOut, normal: vec3<f32>, slot: u32, sample_index: u32) -> f32 {
    let record = rt_rate_metadata(input);
    if (record.x == 0u || slot >= 32u || sample_index >= 4u) { return -1.0; }
    if (dot(normalize(normal), rt_rate_normal(record.x)) < 0.9995) { return -1.0; }
    if (sample_index == 0u) { return f32((record.w >> slot) & 1u); }
    let masks = textureLoad(rt_local_rate_cache, vec2<i32>(input.clip_position.xy), 1, 0);
    return f32((masks[sample_index] >> slot) & 1u);
}
