//! Exact whole-surface draw ownership. This is a cold-path admission check,
//! not a shader-name or a bounding-box suppression heuristic.
use crate::scene::{DrawBatch, DrawClass};
use jka_river::river_promotion::Plan;
use std::collections::{BTreeMap, BTreeSet};

fn supported_stage(b: &DrawBatch) -> bool {
    // Depth-writing/cutout/opaque stages may enter indirect draws, shadow maps
    // and RT geometry. Keep those authored until those consumers are ported.
    b.pipeline.class == DrawClass::Transparent
        && !b.pipeline.depth_write
        && !b.pipeline.depth_equal
        && !b.pipeline.depth_test_disabled
        && !b.pipeline.auto_sprite
        && !b.pipeline.auto_sprite2
        && !b.pipeline.deform_move
        && !b.planar_reflection
        && !b.planar_environment_candidate
        && !b.glass_primary
        && !b.authored_glow
        && !b.force_sight_only
        && b.authored_ocean.is_none()
        && b.additive_companions.iter().all(supported_stage)
}

fn ranges_for_body(
    plan: &Plan,
    batches: &[DrawBatch],
    triangle_surfaces: &[u32],
) -> Result<BTreeSet<(u32, u32)>, &'static str> {
    let expected: BTreeMap<u32, u32> = plan
        .surfaces
        .iter()
        .map(|&(surface, _, shader)| (surface as u32, shader as u32))
        .collect();
    let mut seen = BTreeSet::new();
    let mut ranges = BTreeSet::new();
    for b in batches {
        let range = &b.vertices;
        let Some(ids) = triangle_surfaces.get(range.start as usize / 3..range.end as usize / 3)
        else {
            if expected
                .values()
                .any(|&shader| shader == b.bsp_shader_index)
            {
                return Err("missing source surface metadata");
            }
            continue;
        };
        if !ids.iter().any(|id| expected.contains_key(id)) {
            continue;
        }
        if range.start % 3 != 0 || range.end % 3 != 0 || !supported_stage(b) {
            return Err("unsupported authored draw stage");
        }
        if !ids
            .iter()
            .all(|id| expected.get(id) == Some(&b.bsp_shader_index))
        {
            return Err("batch mixes authored river with other geometry");
        }
        seen.extend(ids.iter().copied());
        ranges.insert((range.start, range.end));
    }
    if seen.len() != expected.len() || ranges.is_empty() {
        return Err("not every river surface has an exact draw owner");
    }
    Ok(ranges)
}

pub(in crate::scene) fn admit(
    plans: &mut [Plan],
    coarse: &[DrawBatch],
    full: &[DrawBatch],
    triangle_surfaces: &[u32],
) {
    for plan in plans {
        if !plan.replace {
            continue;
        }
        if plan.surfaces.iter().any(|&(_, model, _)| model != 0) {
            plan.replace = false;
            eprintln!(
                "[RIVER PROMOTION] {}: inline entity remains preview-only",
                plan.id
            );
            continue;
        }
        let result = ranges_for_body(plan, coarse, triangle_surfaces).and_then(|mut ranges| {
            if !full.is_empty() {
                ranges.extend(ranges_for_body(plan, full, triangle_surfaces)?);
            }
            Ok(ranges)
        });
        match result {
            Ok(ranges) => plan.world_ranges = ranges.into_iter().collect(),
            Err(reason) => {
                plan.replace = false;
                eprintln!(
                    "[RIVER PROMOTION] {}: authored draw retained: {reason}",
                    plan.id
                );
            }
        }
    }
}
