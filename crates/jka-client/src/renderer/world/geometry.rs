//! World geometry.
use crate::renderer::{
    scene, DrawBatch, DrawClass, GpuCullRecord, GpuVertex, InlineModelInstance, WorldBatch,
    WorldBatchSet, WorldDrawGroup,
};

pub(in crate::renderer) fn billboard_radius(vertex: &GpuVertex) -> f32 {
    if vertex.normal[2] == -1337.0 { vertex.normal[0].hypot(vertex.normal[1]) }
    else if vertex.lightmap_uv[1] == -1337.0 { vertex.lightmap_uv[0].abs() }
    else if vertex.normal[2] > 2048.0 { (vertex.normal[2] - 8192.0).abs() }
    else { 0.0 }
}

fn pose_inline_vertex(vertex: &GpuVertex, pose: InlineModelInstance) -> GpuVertex {
    let mut out = *vertex;
    out.position = pose.transform_point(vertex.position);
    // autoSprite's normal is signed billboard offsets and a tag, not a normal.
    // TaystJK cancels the entity rotation when constructing camera-facing axes.
    if vertex.normal[2] == -1337.0 { return out; }
    if vertex.normal[2] > 2048.0 {
        // Lightmapped autoSprite2 stores an octahedral long axis plus width/tag.
        let [x,y] = [vertex.normal[0],vertex.normal[1]];
        let mut axis = [x,y,1.0-x.abs()-y.abs()];
        if axis[2] < 0.0 {
            axis[0]=(1.0-y.abs())*x.signum();
            axis[1]=(1.0-x.abs())*y.signum();
        }
        let axis = pose.transform_normal(axis);
        let sum = axis.iter().map(|v| v.abs()).sum::<f32>().max(1e-20);
        let n = axis.map(|v| v/sum);
        let oct = if n[2]>=0.0 {[n[0],n[1]]}
            else {[(1.0-n[1].abs())*n[0].signum(),(1.0-n[0].abs())*n[1].signum()]};
        out.normal = [oct[0],oct[1],vertex.normal[2]];
    } else { out.normal = pose.transform_normal(vertex.normal); }
    out
}

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
            let out = pose_inline_vertex(vertex, pose);
            let radius = billboard_radius(&out);
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(out.position[axis] - radius);
                maximum[axis] = maximum[axis].max(out.position[axis] + radius);
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
                    *out = pose_inline_vertex(vertex, pose);
                    let radius = billboard_radius(out);
                    for axis in 0..3 {
                        minimum[axis] = minimum[axis].min(out.position[axis] - radius);
                        maximum[axis] = maximum[axis].max(out.position[axis] + radius);
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
    // Billboard positions contain centers; their packed offsets must contribute
    // to the bounds used by both CPU frustum checks and GPU indirect culling.
    let (minimum, maximum) = scene::auto4_source_bounds(source, vertices);
    GpuCullRecord {
        minimum: [minimum[0], minimum[1], minimum[2], 0.0],
        maximum: [maximum[0], maximum[1], maximum[2], 0.0],
        draw: [
            index_range.end.saturating_sub(index_range.start),
            index_range.start,
            u32::from(source.pipeline.class == DrawClass::Sky),
            u32::from(source.force_sight_only) << 9,
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
