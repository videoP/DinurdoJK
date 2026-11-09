//! Runtime-only world draw submission policy. No GPU layout, PVS recipe,
//! draw-compaction membership, or material semantics are mutated.
use crate::renderer::{
    same_world_surface, BlendMode, DrawClass, WorldBatch,
    WorldPipelineVariant,
};

#[derive(Default, Clone, Copy)]
pub(super) struct WorldSortStats {
    pub candidates: u32,
    pub runs: u32,
    pub moved: u32,
    pub binds_before: u32,
    pub binds_after: u32,
}

/// Sorting only inside contiguous runs that are independent, depth-writing
/// stages of the SAME complete-shader sort bucket. A None is a hard barrier.
/// This is intentionally not a depth/front-to-back or transparency sort.
fn reorder_safe_runs<K: Copy + Ord>(
    indices: &[usize], keys: &[Option<(i32, DrawClass, K)>],
) -> (Vec<usize>, WorldSortStats) {
    debug_assert_eq!(indices.len(), keys.len());
    let mut ordered = indices.to_vec();
    let mut stats = WorldSortStats::default();
    let mut start = 0;
    while start < keys.len() {
        let Some((sort, class, _)) = keys[start] else {
            start += 1;
            continue;
        };
        let mut end = start + 1;
        while end < keys.len() && keys[end].is_some_and(|(s, c, _)| s == sort && c == class) {
            end += 1;
        }
        stats.candidates = stats.candidates.saturating_add((end - start) as u32);
        let mut before = None;
        for position in start..end {
            let key = keys[position].unwrap().2;
            if before != Some(key) {
                stats.binds_before = stats.binds_before.saturating_add(1);
                before = Some(key);
            }
        }
        if end - start > 1 {
            stats.runs = stats.runs.saturating_add(1);
            let mut positions = (start..end).collect::<Vec<_>>();
            // Stable: original material order is kept for matching PSOs.
            positions.sort_by_key(|&position| keys[position].unwrap().2);
            let mut after = None;
            for (destination, &source) in (start..end).zip(positions.iter()) {
                ordered[destination] = indices[source];
                if destination != source {
                    stats.moved = stats.moved.saturating_add(1);
                }
                let key = keys[source].unwrap().2;
                if after != Some(key) {
                    stats.binds_after = stats.binds_after.saturating_add(1);
                    after = Some(key);
                }
            }
        } else {
            stats.binds_after = stats.binds_after.saturating_add(1);
        }
        start = end;
    }
    (ordered, stats)
}

/// A batch is sortable ONLY when every material-side ordering dependency is
/// absent, including an adjacent authored sibling stage on the same surface.
/// This excludes the most common legacy JKA depth-equal/multipass hazards.
fn safe_opaque_stage(batches: &[WorldBatch], indices: &[usize], position: usize) -> bool {
    let batch = &batches[indices[position]];
    let material = &batch.source;
    let pipeline = material.pipeline;
    if batch.is_inline_entity || batch.ocean_replaced || batch.ocean_clipmap.is_some()
        || material.late_sorted_material || material.water || material.planar_reflection
        || material.planar_environment_candidate || material.authored_ocean.is_some()
        || material.opaque_composite_before_clouds
        || material.legacy_srgb_overlay.is_some() || !material.additive_companions.is_empty()
        || material.authored_environment_composite
        || material.fog[3] > 0.001 || material.fog_is_global
        || pipeline.legacy_srgb_three_stage || pipeline.additive_fusion_count > 0
        || (pipeline.auto_sprite || pipeline.auto_sprite2 || pipeline.deform_move) || pipeline.offset || pipeline.depth_equal
        || !pipeline.depth_write || pipeline.blend != BlendMode::Opaque
        || !matches!(pipeline.class, DrawClass::Opaque | DrawClass::Mask)
    {
        return false;
    }
    let shares_geometry = |other: usize| same_world_surface(batch, &batches[indices[other]]);
    !(position > 0 && shares_geometry(position - 1)
        || position + 1 < indices.len() && shares_geometry(position + 1))
}

pub(super) fn pipeline_order(
    batches: &[WorldBatch], indices: &[usize], variant: &WorldPipelineVariant,
) -> (Vec<usize>, WorldSortStats) {
    let keys = indices.iter().enumerate().map(|(position, &index)| {
        let batch = &batches[index];
        safe_opaque_stage(batches, indices, position).then(|| (
            batch.source.authored_sort_priority,
            batch.source.pipeline.class,
            variant.base_pipeline(batch).0,
        ))
    }).collect::<Vec<_>>();
    reorder_safe_runs(indices, &keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_order_reduces_binds_only_within_same_sort_and_class() {
        let indices = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        let keys = [
            Some((3072, DrawClass::Opaque, 1_u8)),
            Some((3072, DrawClass::Opaque, 2)),
            Some((3072, DrawClass::Opaque, 1)),
            Some((3072, DrawClass::Opaque, 2)),
            None, // a multi-stage / blended dependency
            Some((3072, DrawClass::Opaque, 2)),
            Some((3072, DrawClass::Opaque, 1)),
            Some((5120, DrawClass::Opaque, 1)), // a different authored sort
            Some((5120, DrawClass::Mask, 2)), // a different depth class
        ];
        let (sorted, stats) = reorder_safe_runs(&indices, &keys);
        assert_eq!(sorted, [0, 2, 1, 3, 4, 6, 5, 7, 8]);
        assert_eq!(stats.binds_before, 8);
        assert_eq!(stats.binds_after, 6);
        assert_eq!(stats.moved, 4);
    }

    #[test]
    fn stable_order_is_preserved_when_pipeline_keys_match() {
        let (order, stats) = reorder_safe_runs(
            &[8, 7, 6], &[Some((3072, DrawClass::Opaque, 1_u8)); 3]
        );
        assert_eq!(order, [8, 7, 6]);
        assert_eq!(stats.binds_before, 1);
        assert_eq!(stats.binds_after, 1);
    }
}
