//! World geometry.
use crate::renderer::{
    scene, DrawBatch, DrawClass, GpuCullRecord, GpuVertex, InlineModelInstance, WorldBatch,
    WorldBatchSet, WorldDrawGroup,
};

/// Transform real, present inline model geometry. Hidden models never call
/// this function: they retain their existing vertex storage and are culled
/// by explicit visibility, not collapsed into zero-area triangles.
pub(in crate::renderer) fn posed_inline_vertices(
    base: &[GpuVertex],
    pose: InlineModelInstance,
) -> (Vec<GpuVertex>, [f32; 3], [f32; 3]) {
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    let vertices = base
        .iter()
        .map(|vertex| {
            let mut out = *vertex;
            out.position = pose.transform_point(vertex.position);
            out.normal = pose.transform_normal(vertex.normal);
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

/// Same vertex math as the scalar reference, split into chunks so one large
/// mover can use several workers. Scratch storage survives between frames.
pub(in crate::renderer) fn posed_inline_vertices_workers(
    base: &[GpuVertex],
    pose: InlineModelInstance,
    vertices: &mut Vec<GpuVertex>,
    pool: &rayon::ThreadPool,
) -> ([f32; 3], [f32; 3]) {
    use rayon::prelude::*;
    const CHUNK_VERTICES: usize = 4096;
    let Some(first) = base.first() else {
        vertices.clear();
        return ([0.0; 3], [0.0; 3]);
    };
    vertices.resize(base.len(), *first);
    pool.install(|| {
        base.par_chunks(CHUNK_VERTICES)
            .zip(vertices.par_chunks_mut(CHUNK_VERTICES))
            .map(|(source, destination)| {
                let mut minimum = [f32::INFINITY; 3];
                let mut maximum = [f32::NEG_INFINITY; 3];
                for (vertex, out) in source.iter().zip(destination) {
                    *out = *vertex;
                    out.position = pose.transform_point(vertex.position);
                    out.normal = pose.transform_normal(vertex.normal);
                    for axis in 0..3 {
                        minimum[axis] = minimum[axis].min(out.position[axis]);
                        maximum[axis] = maximum[axis].max(out.position[axis]);
                    }
                }
                (minimum, maximum)
            })
            .reduce(|| ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |a, b| {
                (std::array::from_fn(|axis| a.0[axis].min(b.0[axis])),
                 std::array::from_fn(|axis| a.1[axis].max(b.1[axis])))
            })
    })
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
    if source.foliage_wind {
        for axis in 0..3 {
            minimum[axis] -= crate::foliage::BOUNDS_PADDING;
            maximum[axis] += crate::foliage::BOUNDS_PADDING;
        }
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
        face_plane: [0.0; 4],
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
