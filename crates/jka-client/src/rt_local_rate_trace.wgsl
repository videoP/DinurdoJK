@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> rt_shadow_settings: RtSunShadowSettings;
@group(1) @binding(1) var rt_full_depth: texture_2d<f32>;
@group(1) @binding(2) var rt_motion_vectors: texture_2d<f32>;
@group(1) @binding(3) var rt_local_prev: texture_2d_array<u32>;
@group(1) @binding(4) var rt_local_out: texture_storage_2d_array<rgba32uint, write>;
@group(1) @binding(5) var<uniform> lighting_settings: LightingSettings;
@group(1) @binding(6) var<storage, read> dynamic_lights: array<PointLight>;
@group(1) @binding(7) var<storage, read> light_clusters: array<ClusterRecord>;
@group(1) @binding(8) var rt_local_published: texture_storage_2d_array<rgba32uint, write>;

fn rt_local_store(pixel: vec2<i32>, masks: vec4<u32>, metadata: vec4<u32>) {
    let record = vec4<u32>(metadata.xyz, masks.x);
    textureStore(rt_local_out, pixel, 0, record);
    textureStore(rt_local_published, pixel, 0, record);
    if (rt_sample_count() > 1u) {
        textureStore(rt_local_out, pixel, 1, masks);
        textureStore(rt_local_published, pixel, 1, masks);
    }
}

fn rt_local_point(pixel: vec2<i32>) -> vec3<f32> {
    let p = clamp(pixel, vec2<i32>(0), vec2<i32>(rt_shadow_settings.dimensions.xy) - 1);
    return rt_shadow_world_position(vec2<f32>(p) + 0.5, textureLoad(rt_full_depth, p, 0).x);
}

fn rt_light_hash(hash: u32, light: PointLight, index: u32) -> u32 {
    var h = (hash ^ index) * 16777619u;
    let words = array<vec4<u32>, 5>(bitcast<vec4<u32>>(light.position_radius),
        bitcast<vec4<u32>>(light.color_intensity), bitcast<vec4<u32>>(light.emitter),
        bitcast<vec4<u32>>(light.shape), bitcast<vec4<u32>>(light.shadow));
    for (var i = 0u; i < 5u; i += 1u) {
        h = (h ^ words[i].x) * 16777619u;
        h = (h ^ words[i].y) * 16777619u;
        h = (h ^ words[i].z) * 16777619u;
        h = (h ^ words[i].w) * 16777619u;
    }
    return h;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= rt_shadow_settings.dimensions.xy)) { return; }
    let pixel = vec2<i32>(id.xy);
    let depth = textureLoad(rt_full_depth, pixel, 0).x;
    if (!(depth > 0.0 && depth < 65000.0)) {
        rt_local_store(pixel, vec4<u32>(0u), vec4<u32>(0u)); return;
    }
    let position = rt_local_point(pixel);
    let left = position - rt_local_point(pixel - vec2<i32>(1, 0));
    let right = rt_local_point(pixel + vec2<i32>(1, 0)) - position;
    let up = position - rt_local_point(pixel - vec2<i32>(0, 1));
    let down = rt_local_point(pixel + vec2<i32>(0, 1)) - position;
    let dx = select(right, left, dot(left, left) < dot(right, right) && dot(left, left) > 1e-10);
    let dy = select(down, up, dot(up, up) < dot(down, down) && dot(up, up) > 1e-10);
    let cross_normal = cross(dx, dy);
    if (dot(cross_normal, cross_normal) <= 1e-12) {
        // Receiver performs direct queries at degenerate silhouettes.
        rt_local_store(pixel, vec4<u32>(0u), vec4<u32>(0u)); return;
    }
    var normal = normalize(cross_normal);
    normal *= select(-1.0, 1.0, dot(normal, camera.camera_pos_time.xyz - position) >= 0.0);
    var input: VertexOut;
    input.clip_position = vec4<f32>(vec2<f32>(pixel) + 0.5, 0.0, 1.0);
    input.world_position = position;
    let cluster = cluster_for_fragment(input);
    if (cluster.count == 0u) { rt_local_store(pixel, vec4<u32>(0u), vec4<u32>(0u)); return; }
    var signature = u32(rt_sample_count()) + 8u * rt_shadow_settings.flags.z;
    for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {
        signature = rt_light_hash(signature, dynamic_lights[cluster.indices[i]], cluster.indices[i]);
    }
    let rate = max(rt_shadow_settings.dimensions.w, 1u);
    let phase = rt_shadow_settings.dimensions.z % rate;
    let cell = (id.x & 1u) + 2u * (id.y & 1u);
    let scheduled = select((id.x + id.y) % 2u == phase, cell == phase, rate == 4u);
    var trace = scheduled || rt_shadow_settings.flags.y == 0u;
    let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(rt_shadow_settings.dimensions.xy)
        - textureLoad(rt_motion_vectors, pixel, 0).xy;
    var old_masks = vec4<u32>(0u);
    var old_meta = vec4<u32>(0u);
    if (!trace && all(uv >= vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0))) {
        let p = vec2<i32>(uv * vec2<f32>(rt_shadow_settings.dimensions.xy));
        old_meta = textureLoad(rt_local_prev, p, 0, 0);
        old_masks = vec4<u32>(old_meta.w, 0u, 0u, 0u);
        if (rt_sample_count() > 1u) { old_masks = textureLoad(rt_local_prev, p, 1, 0); }
        let expected = distance(rt_shadow_settings.prev_camera_pos.xyz, position);
        trace = old_meta.x == 0u || old_meta.y != signature
            || abs(rt_rate_depth(old_meta.x) - expected) > max(depth * 0.001, 0.05)
            || dot(normal, rt_rate_normal(old_meta.x)) < 0.9995;
        // Compare only neighbours with this same light-list signature. Different
        // visibility at a boundary forces fresh rays for moving caster edges.
        let offsets = array<vec2<i32>, 4>(vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1));
        for (var j = 0u; j < 4u && !trace; j += 1u) {
            let q = clamp(p + offsets[j], vec2<i32>(0), vec2<i32>(rt_shadow_settings.dimensions.xy) - 1);
            let neighbour = textureLoad(rt_local_prev, q, 0, 0);
            var neighbour_masks = vec4<u32>(neighbour.w, 0u, 0u, 0u);
            if (rt_sample_count() > 1u) { neighbour_masks = textureLoad(rt_local_prev, q, 1, 0); }
            if (neighbour.y == signature && any(neighbour_masks != old_masks)) { trace = true; }
        }
    } else { trace = true; }
    var masks = old_masks;
    var seed = old_meta.z;
    if (trace) {
        seed = rt_emitter_seed(input);
        masks = vec4<u32>(0u);
        for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {
            let light = dynamic_lights[cluster.indices[i]];
            // Static map emitters use raster shadows at the receiver. Their
            // cache bits are unused; never trace them in this compute pass.
            if (light.shadow.w < 0.5) { continue; }
            let count = rt_local_sample_count(light);
            for (var s = 0u; s < count; s += 1u) {
                let sampled = rt_sample_local_emitter_seed(light, input, cluster.indices[i], s, count, seed);
                let visibility = ray_traced_local_shadow_visibility(sampled, position, normal);
                if (visibility > 0.5) { masks[s] |= 1u << i; }
            }
        }
    }
    rt_local_store(pixel, masks, vec4<u32>(rt_rate_pack_surface(depth, normal), signature, seed, 0u));
}
