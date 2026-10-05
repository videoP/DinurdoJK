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
@group(1) @binding(9) var<storage, read_write> early_draws: array<DrawIndexedIndirectArgs>;

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

// Hi-Z occlusion test against the power-of-two depth pyramid.
//
// Same scheme as Bevy's two-phase occlusion culling: project the AABB, pick the
// pyramid level whose texels are at least as large as the footprint, and load
// the (at most 2x2) texels it touches. Depth is the reversed-Z hardware depth
// (near -> 1, far -> 0) and the pyramid stores the MINIMUM (farthest) value, so
// the box is hidden when even its nearest point is farther than the farthest
// occluder anywhere in its footprint.
fn occluded_by_hiz(record: CullRecord) -> bool {
    if (record.draw.z != 0u || settings.viewport_mips_flags.w == 0u) {
        return false;
    }

    var uv_min = vec2<f32>(1.0, 1.0);
    var uv_max = vec2<f32>(0.0, 0.0);
    // Reversed-Z: the largest corner depth is the nearest point of the box.
    var nearest_depth = 0.0;
    for (var i = 0u; i < 8u; i += 1u) {
        let p = camera.view_proj * vec4<f32>(corner(record, i), 1.0);
        // A corner at or behind the eye makes the screen rectangle unbounded.
        if (p.w <= 0.0001) {
            return false;
        }
        let inv_w = 1.0 / p.w;
        let uv = vec2<f32>(p.x * inv_w * 0.5 + 0.5, 0.5 - p.y * inv_w * 0.5);
        uv_min = min(uv_min, uv);
        uv_max = max(uv_max, uv);
        nearest_depth = max(nearest_depth, p.z * inv_w);
    }
    // A box crossing the near plane is clipped there; no fragment is nearer.
    nearest_depth = min(nearest_depth, 1.0);

    uv_min = clamp(uv_min, vec2<f32>(0.0), vec2<f32>(1.0));
    uv_max = clamp(uv_max, vec2<f32>(0.0), vec2<f32>(1.0));
    let base_dims = vec2<f32>(textureDimensions(hi_z, 0));
    let texel_span = max(
        (uv_max.x - uv_min.x) * base_dims.x,
        (uv_max.y - uv_min.y) * base_dims.y,
    );
    let max_mip = max(settings.viewport_mips_flags.z, 1u) - 1u;
    let mip = min(u32(max(0.0, ceil(log2(max(texel_span, 1.0))))), max_mip);
    let dims = textureDimensions(hi_z, i32(mip));
    let max_coord = max(dims, vec2<u32>(1u)) - vec2<u32>(1u);

    // The level's texels are at least as large as the footprint, so the four
    // corner texels cover every texel the rectangle can touch.
    let s0 = min(vec2<u32>(uv_min * vec2<f32>(dims)), max_coord);
    let s1 = min(vec2<u32>(vec2<f32>(uv_max.x, uv_min.y) * vec2<f32>(dims)), max_coord);
    let s2 = min(vec2<u32>(vec2<f32>(uv_min.x, uv_max.y) * vec2<f32>(dims)), max_coord);
    let s3 = min(vec2<u32>(uv_max * vec2<f32>(dims)), max_coord);
    let occluder_depth = min(
        min(textureLoad(hi_z, vec2<i32>(s0), i32(mip)).x, textureLoad(hi_z, vec2<i32>(s1), i32(mip)).x),
        min(textureLoad(hi_z, vec2<i32>(s2), i32(mip)).x, textureLoad(hi_z, vec2<i32>(s3), i32(mip)).x),
    );

    // Depth is ~near/distance, so a relative margin is a relative distance
    // margin. It absorbs the rounding difference between this matrix multiply
    // and the rasteriser's. An undrawn region reads 0 and never culls.
    return nearest_depth < occluder_depth * 0.9998;
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
    // Sky draw records are a background domain. Their authored BSP brush bounds
    // can extend beyond the distanceCull-limited projection far plane, but the
    // sky vertex path renders them at far depth. Match the CPU submission path
    // by exempting sky from geometric frustum rejection; otherwise GPU-driven
    // culling can zero instance_count before the sky vertex shader ever runs.
    if (record.draw.z == 0u && outside_frustum(record)) {
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
        // instance_count doubles as last-frame visibility: the early Hi-Z
        // prepass of the next frame draws exactly the batches left at 1.
        draws[index].index_count = 0u;
        draws[index].instance_count = 0u;
        draws[index].first_index = selected_draw.y;
        draws[index].base_vertex = 0;
        draws[index].first_instance = 0u;
    }
}

// Early Hi-Z phase: build the draw list for the occluder prepass from what was
// visible last frame (`instance_count` was left at 1 by `cs_main`) and still
// passes the frustum. No Hi-Z test here: there is no pyramid yet, and drawing a
// superset of the true occluders only makes the pyramid, and so the later
// culling, more conservative, never wrong.
@compute @workgroup_size(64, 1, 1)
fn cs_early(@builtin(global_invocation_id) gid: vec3<u32>) {
    let active_index = gid.x;
    if (active_index >= settings.active_compaction.x || active_index >= arrayLength(&active_indices)) {
        return;
    }
    let index = active_indices[active_index];
    if (index >= arrayLength(&records) || index >= arrayLength(&draws) || index >= arrayLength(&early_draws)) {
        return;
    }

    let record = records[index];
    let draw_it = draws[index].instance_count != 0u
        && record.draw.z == 0u
        && !outside_frustum(record);
    early_draws[index].index_count = select(0u, record.draw.x, draw_it);
    early_draws[index].instance_count = select(0u, 1u, draw_it);
    early_draws[index].first_index = record.draw.y;
    early_draws[index].base_vertex = 0;
    early_draws[index].first_instance = 0u;
}
