//! Visibility pvs.
use crate::renderer::{
    auto4_accept_collapse, scene, Auto4CollapseRequest, Auto4GeometryState, Auto4PlanStats, Camera,
    PreparedPortalPlanBatchRef, PvsMode, WorldBatch, WorldGpu,
};

pub(in crate::renderer) fn visible_batches_by_cluster(
    batches: &[WorldBatch],
    vis: &jka_assets::bsp::Visibility,
) -> Vec<Vec<usize>> {
    let mut by_cluster = vec![Vec::new(); vis.clusters];
    for (batch_index, batch) in batches.iter().enumerate() {
        if batch.source.pvs_signature.is_empty() {
            for list in &mut by_cluster {
                list.push(batch_index);
            }
            continue;
        }
        for (cluster, list) in by_cluster.iter_mut().enumerate() {
            let word = cluster / 64;
            let bit = cluster % 64;
            if batch
                .source
                .pvs_signature
                .get(word)
                .is_none_or(|value| *value & (1_u64 << bit) != 0)
            {
                list.push(batch_index);
            }
        }
    }
    by_cluster
}

pub(in crate::renderer) fn update_visibility(
    world: &mut WorldGpu,
    camera: &Camera,
) -> Option<usize> {
    let cluster = world
        .visibility
        .as_ref()
        .and_then(|vis| vis.cluster_at(scene::jka_position(camera.position.to_array())));
    world.last_cluster = Some(cluster);
    cluster
}

/// Select the current cluster's AUTO 4 plan. Steady state is one channel poll
/// and two comparisons; the active list is rebuilt only when the cluster, the
/// areaportal mask, or a finished background collapse changes it.
pub(in crate::renderer) fn refresh_auto4_lazy_collapse(
    queue: &wgpu::Queue,
    world: &mut WorldGpu,
    mode: PvsMode,
    area_mask: Option<[u8; 32]>,
    log: bool,
) {
    if mode != PvsMode::Auto || world.auto4_plan_refs.is_empty() {
        return;
    }
    let plan_id = world
        .last_cluster
        .flatten()
        .and_then(|cluster| world.auto4_plan_by_cluster.get(cluster).copied())
        .filter(|&plan_id| plan_id < world.auto4_plan_refs.len());
    let Some(plan_id) = plan_id else {
        if !world.auto4_active_plan.is_empty() {
            world.auto4_active_plan.clear();
            world.active_selection_key = None;
        }
        world.auto4_collapse_cache.current_cluster = None;
        return;
    };
    let cluster = world.last_cluster.flatten().unwrap_or_default();

    let cluster_changed = world.auto4_collapse_cache.current_cluster != Some(cluster);
    let mut changed = cluster_changed || world.auto4_collapse_cache.current_area_mask != area_mask;
    loop {
        let result = match world
            .auto4_collapse_cache
            .worker
            .as_ref()
            .map(|worker| worker.receiver.try_recv())
        {
            Some(Ok(result)) => result,
            _ => break,
        };
        auto4_accept_collapse(queue, world, result, plan_id);
        changed = true;
    }
    if !changed {
        return;
    }
    world.auto4_collapse_cache.current_cluster = Some(cluster);
    world.auto4_collapse_cache.current_area_mask = area_mask;
    // LRU clock: advances per plan change, not per frame.
    world.auto4_collapse_cache.frame = world.auto4_collapse_cache.frame.wrapping_add(1);
    let frame = world.auto4_collapse_cache.frame;

    let refs = std::mem::take(&mut world.auto4_plan_refs[plan_id]);
    let variant_base = world.auto4_variant_base;
    let mut active =
        Vec::with_capacity(refs.len() + variant_base.saturating_sub(world.auto4_always_start));
    let mut stats = Auto4PlanStats::default();
    for reference in &refs {
        let variant = match *reference {
            PreparedPortalPlanBatchRef::Base(index) => {
                if index < variant_base {
                    active.push(index);
                    stats.base_draws += 1;
                }
                continue;
            }
            PreparedPortalPlanBatchRef::Merged(variant) => variant,
        };
        let (Some(members), Some(&geometry)) = (
            world.auto4_variant_members.get(variant),
            world.auto4_variant_geometry.get(variant),
        ) else {
            continue;
        };
        let world_members = members
            .iter()
            .copied()
            .filter(|&member| member < variant_base);

        // A closed areaportal that hides only part of a mixed-area recipe:
        // draw its exact pieces (each area-filtered) until the mask changes.
        let area_split = area_mask.is_some()
            && world
                .auto4_variant_area_mixed
                .get(variant)
                .copied()
                .unwrap_or(false)
            && members.iter().any(|&member| {
                world
                    .full_batches
                    .get(member)
                    .is_some_and(|batch| !batch_area_visible(batch, area_mask.as_ref()))
            });
        if area_split {
            active.extend(world_members);
            stats.area_split_recipes += 1;
            stats.fallback_draws += members.len();
            continue;
        }

        if cluster_changed
            && matches!(
                world.auto4_collapse_cache.states[geometry],
                Auto4GeometryState::Deferred
            )
        {
            world.auto4_collapse_cache.states[geometry] = Auto4GeometryState::Uncached;
        }
        match &mut world.auto4_collapse_cache.states[geometry] {
            Auto4GeometryState::Static => {
                active.push(variant_base + variant);
                stats.physical_draws += 1;
                continue;
            }
            Auto4GeometryState::Ready {
                last_used_frame, ..
            } => {
                *last_used_frame = frame;
                active.push(variant_base + variant);
                stats.physical_draws += 1;
                continue;
            }
            _ => {}
        }
        active.extend(world_members);
        stats.fallback_recipes += 1;
        stats.fallback_draws += members.len();
        if matches!(
            world.auto4_collapse_cache.states[geometry],
            Auto4GeometryState::Uncached
        ) {
            let request = Auto4CollapseRequest {
                geometry,
                members: world.auto4_geometry_members[geometry].clone(),
            };
            let sent = world
                .auto4_collapse_cache
                .worker
                .as_ref()
                .and_then(|worker| worker.sender.as_ref())
                .is_some_and(|sender| sender.try_send(request).is_ok());
            if sent {
                world.auto4_collapse_cache.states[geometry] = Auto4GeometryState::Pending;
                world.auto4_collapse_cache.queued =
                    world.auto4_collapse_cache.queued.saturating_add(1);
            }
        }
    }
    world.auto4_plan_refs[plan_id] = refs;
    active.extend(world.auto4_always_start..variant_base);

    if active != world.auto4_active_plan {
        world.auto4_active_plan = active;
        world.active_selection_key = None;
    }
    let stats_changed = stats != world.auto4_collapse_cache.stats;
    world.auto4_collapse_cache.stats = stats;
    if log && (cluster_changed || stats_changed) {
        let cache = &world.auto4_collapse_cache;
        let mib = |indices: u32| f64::from(indices) * 4.0 / (1024.0 * 1024.0);
        rverbose!(
            3,
            "AUTO 4 plan: cluster {cluster} plan {plan_id}: {} real draw(s) [{} recipe + {} single-piece], {} cold recipe(s) + {} area-split recipe(s) -> {} FULL piece draw(s); cache {:.2}/{:.2} MiB, {} queued / {} completed, {} eviction(s), {} oversize",
            stats.physical_draws + stats.base_draws,
            stats.physical_draws,
            stats.base_draws,
            stats.fallback_recipes,
            stats.area_split_recipes,
            stats.fallback_draws,
            mib(cache.used_indices),
            mib(cache.capacity_indices),
            cache.queued,
            cache.completed,
            cache.evictions,
            cache.oversize,
        );
    }
}

pub(in crate::renderer) struct ActiveBatchSelection<'a> {
    pub(in crate::renderer) batches: &'a [WorldBatch],
    pub(in crate::renderer) indices: &'a [usize],
    /// Number of batches in the representation selected for this cluster before
    /// PVS rejection. This keeps cull diagnostics meaningful for hybrid AUTO
    /// modes even though their backing slice contains coarse and full aliases.
    pub(in crate::renderer) candidate_count: usize,
}

pub(in crate::renderer) fn active_batch_selection(
    world: &WorldGpu,
    mode: PvsMode,
) -> ActiveBatchSelection<'_> {
    let coarse_all = ActiveBatchSelection {
        batches: &world.coarse_batches,
        indices: &world.coarse_all_batches,
        candidate_count: world.coarse_batches.len(),
    };
    if mode == PvsMode::Off {
        return coarse_all;
    }

    let Some(cluster) = world.last_cluster.flatten() else {
        // Outside the BSP tree or a source with no compiled visibility: render
        // conservatively with the lowest-draw-call representation.
        return coarse_all;
    };
    let Some(coarse_visible) = world.coarse_visible_batches_by_cluster.get(cluster) else {
        return coarse_all;
    };

    let coarse = ActiveBatchSelection {
        batches: &world.coarse_batches,
        indices: coarse_visible,
        candidate_count: world.coarse_batches.len(),
    };
    if mode == PvsMode::Minimal || world.full_batches.is_empty() {
        return coarse;
    }

    let Some(full_visible) = world.full_visible_batches_by_cluster.get(cluster) else {
        return coarse;
    };
    let full = ActiveBatchSelection {
        batches: &world.full_batches,
        indices: full_visible,
        candidate_count: world.full_batches.len(),
    };
    match mode {
        PvsMode::Full => full,
        PvsMode::Auto => {
            if world.auto4_batches.is_empty() || world.auto4_active_plan.is_empty() {
                return full;
            }
            ActiveBatchSelection {
                batches: &world.auto4_batches,
                indices: &world.auto4_active_plan,
                candidate_count: world.auto4_batches.len(),
            }
        }
        PvsMode::Off | PvsMode::Minimal => coarse,
    }
}

pub(in crate::renderer) fn legacy_dlight_batch_selection(
    world: &WorldGpu,
    mode: PvsMode,
) -> ActiveBatchSelection<'_> {
    let coarse_all = ActiveBatchSelection {
        batches: &world.coarse_batches,
        indices: &world.coarse_all_batches,
        candidate_count: world.coarse_batches.len(),
    };
    if mode == PvsMode::Off {
        return coarse_all;
    }
    let Some(cluster) = world.last_cluster.flatten() else {
        return coarse_all;
    };
    let Some(indices) = world.coarse_visible_batches_by_cluster.get(cluster) else {
        return coarse_all;
    };
    // Always use the conservative coarse/PVS representation for this redraw.
    // AUTO/FULL/AUTO4 may reorganize or physically collapse index ranges, but
    // depth-EQUAL guarantees that extra conservative triangles cannot light a
    // pixel the material pass did not actually write. This keeps the dlight
    // pass independent of visibility batching while submitting only the small
    // authored surface runs whose masks are non-zero.
    ActiveBatchSelection {
        batches: &world.coarse_batches,
        indices,
        candidate_count: world.coarse_batches.len(),
    }
}

pub(in crate::renderer) fn effective_area_mask<'a>(
    world: &WorldGpu,
    mode: PvsMode,
    area_mask: Option<&'a [u8; 32]>,
) -> Option<&'a [u8; 32]> {
    if mode == PvsMode::Off || world.last_cluster.flatten().is_none() {
        None
    } else {
        area_mask
    }
}

pub(in crate::renderer) fn area_signature_visible(
    signature: &[u64; 4],
    area_mask: Option<&[u8; 32]>,
) -> bool {
    let Some(area_mask) = area_mask else {
        return true;
    };
    if signature.iter().all(|&word| word == 0) {
        // Unknown/not-applicable area membership remains conservatively visible.
        return true;
    }
    for (word_index, &membership) in signature.iter().enumerate() {
        let start = word_index * 8;
        let closed = u64::from_le_bytes(area_mask[start..start + 8].try_into().unwrap());
        if membership & !closed != 0 {
            return true;
        }
    }
    false
}

pub(in crate::renderer) fn batch_area_visible(
    batch: &WorldBatch,
    area_mask: Option<&[u8; 32]>,
) -> bool {
    area_signature_visible(&batch.source.area_signature, area_mask)
}

pub(in crate::renderer) fn update_active_cull_indices(
    queue: &wgpu::Queue,
    world: &mut WorldGpu,
    mode: PvsMode,
    area_mask: Option<&[u8; 32]>,
    debug_enabled: bool,
) -> bool {
    let cluster_key = if mode == PvsMode::Off {
        None
    } else {
        world.last_cluster.flatten()
    };
    let area_mask = effective_area_mask(world, mode, area_mask);
    let key = (mode, cluster_key, area_mask.copied());
    if world.active_selection_key == Some(key) {
        return false;
    }

    let (indices, debug_reasons) = {
        let active = active_batch_selection(world, mode);
        let indices = active
            .indices
            .iter()
            .filter(|&&batch_index| batch_area_visible(&active.batches[batch_index], area_mask))
            .map(|&batch_index| active.batches[batch_index].cull_index)
            .collect::<Vec<_>>();
        let debug_reasons = debug_enabled.then(|| {
            let mut reasons = vec![0_u32; world.cull_record_count.max(1)];
            // Anything in the selected representation but absent from the PVS
            // active list was rejected before GPU frustum/Hi-Z culling.
            for batch in active.batches {
                if let Some(reason) = reasons.get_mut(batch.cull_index as usize) {
                    *reason = 3;
                }
            }
            for &batch_index in active.indices {
                let cull_index = active.batches[batch_index].cull_index as usize;
                if let Some(reason) = reasons.get_mut(cull_index) {
                    *reason = if batch_area_visible(&active.batches[batch_index], area_mask) {
                        0
                    } else {
                        4
                    };
                }
            }
            reasons
        });
        (indices, debug_reasons)
    };
    if !indices.is_empty() {
        queue.write_buffer(
            &world.active_cull_indices_buffer,
            0,
            bytemuck::cast_slice(&indices),
        );
    }
    if let Some(reasons) = debug_reasons {
        queue.write_buffer(
            &world.cull_debug_reason_buffer,
            0,
            bytemuck::cast_slice(&reasons),
        );
    }
    world.active_cull_count = u32::try_from(indices.len()).unwrap_or(u32::MAX);
    world.active_selection_key = Some(key);
    true
}
