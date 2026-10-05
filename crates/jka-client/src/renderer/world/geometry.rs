//! World geometry.
use crate::renderer::{
    scene, DrawBatch, DrawClass, GpuCullRecord, GpuVertex, InlineModelInstance, WorldBatch,
    WorldBatchSet, WorldDrawGroup,
};

/// Pose an inline model's compiled vertices. `None` (not in the snapshot)
/// collapses every vertex onto one point: zero-area triangles rasterize
/// nothing, and the returned bounds are that point.
pub(in crate::renderer) fn posed_inline_vertices(
    base: &[GpuVertex],
    pose: Option<InlineModelInstance>,
) -> (Vec<GpuVertex>, [f32; 3], [f32; 3]) {
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    let hidden_point = base.first().map_or([0.0; 3], |vertex| vertex.position);
    let vertices = base
        .iter()
        .map(|vertex| {
            let mut out = *vertex;
            match pose {
                Some(pose) => {
                    out.position = pose.transform_point(vertex.position);
                    out.normal = pose.transform_normal(vertex.normal);
                }
                None => out.position = hidden_point,
            }
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(out.position[axis]);
                maximum[axis] = maximum[axis].max(out.position[axis]);
            }
            out
        })
        .collect::<Vec<_>>();
    if vertices.is_empty() {
        return (vertices, [0.0; 3], [0.0; 3]);
    }
    (vertices, minimum, maximum)
}

pub(in crate::renderer) fn make_cull_record(
    source: &DrawBatch,
    vertices: &[GpuVertex],
    index_range: &std::ops::Range<u32>,
) -> GpuCullRecord {
    let start = usize::try_from(source.vertices.start).unwrap_or(usize::MAX);
    let end = usize::try_from(source.vertices.end)
        .unwrap_or(usize::MAX)
        .min(vertices.len());
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    if start < end {
        for vertex in &vertices[start..end] {
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex.position[axis]);
                maximum[axis] = maximum[axis].max(vertex.position[axis]);
            }
        }
    } else {
        minimum = [0.0; 3];
        maximum = [0.0; 3];
    }
    GpuCullRecord {
        minimum: [minimum[0], minimum[1], minimum[2], 0.0],
        maximum: [maximum[0], maximum[1], maximum[2], 0.0],
        draw: [
            index_range.end.saturating_sub(index_range.start),
            index_range.start,
            u32::from(source.pipeline.class == DrawClass::Sky),
            0,
        ],
        compact: [u32::MAX, 0, 0, 0],
    }
}

pub(in crate::renderer) fn same_draw_state(a: &DrawBatch, b: &DrawBatch) -> bool {
    scene::draw_batches_share_state(a, b)
}

pub(in crate::renderer) fn build_draw_compaction_groups(
    records: &mut [GpuCullRecord],
    coarse_batches: &mut [WorldBatch],
    full_batches: &mut [WorldBatch],
) -> Vec<WorldDrawGroup> {
    struct CandidateGroup {
        representative_set: WorldBatchSet,
        representative_index: usize,
        members: Vec<(WorldBatchSet, usize)>,
    }

    let mut candidates = Vec::<CandidateGroup>::new();
    for set in [WorldBatchSet::Coarse, WorldBatchSet::Full] {
        let batches: &[WorldBatch] = match set {
            WorldBatchSet::Coarse => coarse_batches,
            WorldBatchSet::Full => full_batches,
        };
        for (batch_index, batch) in batches.iter().enumerate() {
            // Transparent surfaces require their authored ordering, and sky
            // stages are cheap enough to keep on the conservative path. GPU
            // compaction targets the large opaque/masked world workload.
            if !matches!(
                batch.source.pipeline.class,
                DrawClass::Opaque | DrawClass::Mask
            ) {
                continue;
            }
            if let Some(group) = candidates.iter_mut().find(|group| {
                if group.representative_set != set {
                    return false;
                }
                let representative = match group.representative_set {
                    WorldBatchSet::Coarse => &coarse_batches[group.representative_index],
                    WorldBatchSet::Full => &full_batches[group.representative_index],
                };
                same_draw_state(&representative.source, &batch.source)
            }) {
                group.members.push((set, batch_index));
            } else {
                candidates.push(CandidateGroup {
                    representative_set: set,
                    representative_index: batch_index,
                    members: vec![(set, batch_index)],
                });
            }
        }
    }

    let mut groups = Vec::new();
    let mut output_base = 0_u32;
    for candidate in candidates
        .into_iter()
        .filter(|group| group.members.len() >= 2)
    {
        let group_index = u32::try_from(groups.len()).unwrap_or(u32::MAX);
        let max_count = u32::try_from(candidate.members.len()).unwrap_or(u32::MAX);
        for (set, batch_index) in &candidate.members {
            let batch = match set {
                WorldBatchSet::Coarse => &mut coarse_batches[*batch_index],
                WorldBatchSet::Full => &mut full_batches[*batch_index],
            };
            batch.compact_group = Some(group_index);
            if let Some(record) = records.get_mut(batch.cull_index as usize) {
                record.compact = [group_index, output_base, 1, 0];
            }
        }
        groups.push(WorldDrawGroup {
            representative_set: candidate.representative_set,
            representative_index: candidate.representative_index,
            output_base,
            max_count,
        });
        output_base = output_base.saturating_add(max_count);
    }
    groups
}
