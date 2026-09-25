struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};

struct CullRecord {
    minimum: vec4<f32>,
    maximum: vec4<f32>,
    draw: vec4<u32>,
    compact: vec4<u32>,
};

struct DrawIndexedIndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};

struct CullSettings {
    viewport_mips_flags: vec4<u32>,
    active_compaction: vec4<u32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<storage, read> records: array<CullRecord>;
@group(1) @binding(1) var<storage, read_write> draws: array<DrawIndexedIndirectArgs>;
@group(1) @binding(2) var hi_z: texture_2d<f32>;
@group(1) @binding(3) var<uniform> settings: CullSettings;
@group(1) @binding(4) var<storage, read> active_indices: array<u32>;
@group(1) @binding(5) var<storage, read_write> compact_draws: array<DrawIndexedIndirectArgs>;
@group(1) @binding(6) var<storage, read_write> compact_counts: array<atomic<u32>>;
@group(1) @binding(7) var<storage, read_write> debug_reasons: array<u32>;
@group(1) @binding(8) var<storage, read_write> debug_counts: array<atomic<u32>, 4>;

fn corner(record: CullRecord, index: u32) -> vec3<f32> {
    return vec3<f32>(
        select(record.minimum.x, record.maximum.x, (index & 1u) != 0u),
        select(record.minimum.y, record.maximum.y, (index & 2u) != 0u),
        select(record.minimum.z, record.maximum.z, (index & 4u) != 0u),
    );
}

fn outside_frustum(record: CullRecord) -> bool {
    var all_left = true;
    var all_right = true;
    var all_below = true;
    var all_above = true;
    var all_near = true;
    var all_far = true;
    for (var i = 0u; i < 8u; i += 1u) {
        let p = camera.view_proj * vec4<f32>(corner(record, i), 1.0);
        all_left = all_left && (p.x < -p.w);
        all_right = all_right && (p.x > p.w);
        all_below = all_below && (p.y < -p.w);
        all_above = all_above && (p.y > p.w);
        all_near = all_near && (p.z < 0.0);
        all_far = all_far && (p.z > p.w);
    }
    return all_left || all_right || all_below || all_above || all_near || all_far;
}

fn occluded_by_hiz(record: CullRecord) -> bool {
    if (record.draw.z != 0u || settings.viewport_mips_flags.w == 0u) {
        return false;
    }

    var uv_min = vec2<f32>(1.0, 1.0);
    var uv_max = vec2<f32>(0.0, 0.0);
    var can_test = true;
    for (var i = 0u; i < 8u; i += 1u) {
        let p = camera.view_proj * vec4<f32>(corner(record, i), 1.0);
        if (p.w <= 0.0001) {
            can_test = false;
            continue;
        }
        let ndc = p.xy / p.w;
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        uv_min = min(uv_min, uv);
        uv_max = max(uv_max, uv);
    }
    if (!can_test) {
        return false;
    }

    uv_min = clamp(uv_min, vec2<f32>(0.0), vec2<f32>(1.0));
    uv_max = clamp(uv_max, vec2<f32>(0.0), vec2<f32>(1.0));
    let pixel_span = max(
        (uv_max.x - uv_min.x) * f32(max(settings.viewport_mips_flags.x, 1u)),
        (uv_max.y - uv_min.y) * f32(max(settings.viewport_mips_flags.y, 1u)),
    );
    let max_mip = max(settings.viewport_mips_flags.z, 1u) - 1u;
    let mip = min(u32(max(0.0, ceil(log2(max(pixel_span, 1.0))))), max_mip);
    let dims = textureDimensions(hi_z, i32(mip));
    let max_coord = max(dims, vec2<u32>(1u)) - vec2<u32>(1u);

    let sample_uv0 = uv_min;
    let sample_uv1 = vec2<f32>(uv_max.x, uv_min.y);
    let sample_uv2 = vec2<f32>(uv_min.x, uv_max.y);
    let sample_uv3 = uv_max;

    var max_depth = 0.0;
    let s0 = min(vec2<u32>(sample_uv0 * vec2<f32>(dims)), max_coord);
    let s1 = min(vec2<u32>(sample_uv1 * vec2<f32>(dims)), max_coord);
    let s2 = min(vec2<u32>(sample_uv2 * vec2<f32>(dims)), max_coord);
    let s3 = min(vec2<u32>(sample_uv3 * vec2<f32>(dims)), max_coord);
    max_depth = max(max_depth, textureLoad(hi_z, vec2<i32>(s0), i32(mip)).x);
    max_depth = max(max_depth, textureLoad(hi_z, vec2<i32>(s1), i32(mip)).x);
    max_depth = max(max_depth, textureLoad(hi_z, vec2<i32>(s2), i32(mip)).x);
    max_depth = max(max_depth, textureLoad(hi_z, vec2<i32>(s3), i32(mip)).x);

    // The linear-depth prepass clears the background to a huge value. Any such
    // sample means the rectangle is not fully covered, so rejecting the batch
    // would not be conservative.
    if (max_depth >= 999999.0) {
        return false;
    }

    let nearest = clamp(camera.camera_pos_time.xyz, record.minimum.xyz, record.maximum.xyz);
    let nearest_depth = distance(camera.camera_pos_time.xyz, nearest);
    return nearest_depth > max_depth + 1.0;
}

@compute @workgroup_size(64, 1, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let active_index = gid.x;
    if (active_index >= settings.active_compaction.x || active_index >= arrayLength(&active_indices)) {
        return;
    }
    let index = active_indices[active_index];
    if (index >= arrayLength(&records) || index >= arrayLength(&draws)) {
        return;
    }

    let record = records[index];
    var reason = 0u;
    var visible = true;
    if (outside_frustum(record)) {
        visible = false;
        reason = 1u;
    } else if (occluded_by_hiz(record)) {
        visible = false;
        reason = 2u;
    }

    // Diagnostics are deliberately dormant in the normal path. When enabled,
    // record the first rejection stage and count it without changing culling.
    if (settings.active_compaction.w != 0u) {
        debug_reasons[index] = reason;
        atomicAdd(&debug_counts[reason], 1u);
    }

    let selected_draw = record.draw.xy;
    if (visible) {
        draws[index].index_count = selected_draw.x;
        draws[index].instance_count = 1u;
        draws[index].first_index = selected_draw.y;
        draws[index].base_vertex = 0;
        draws[index].first_instance = 0u;

        if (settings.active_compaction.z != 0u && record.compact.z != 0u) {
            let group_index = record.compact.x;
            if (group_index < settings.active_compaction.y && group_index < arrayLength(&compact_counts)) {
                let slot = atomicAdd(&compact_counts[group_index], 1u);
                let output_index = record.compact.y + slot;
                if (output_index < arrayLength(&compact_draws)) {
                    compact_draws[output_index].index_count = selected_draw.x;
                    compact_draws[output_index].instance_count = 1u;
                    compact_draws[output_index].first_index = selected_draw.y;
                    compact_draws[output_index].base_vertex = 0;
                    compact_draws[output_index].first_instance = 0u;
                }
            }
        }
    } else {
        draws[index].index_count = 0u;
        draws[index].instance_count = 1u;
        draws[index].first_index = selected_draw.y;
        draws[index].base_vertex = 0;
        draws[index].first_instance = 0u;
    }
}
