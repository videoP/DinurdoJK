//! World inline models.
use crate::renderer::{
    posed_inline_vertices, same_world_surface, weather, world_blend_probe_bucket,
    DrawClass, DrawIndexedIndirectArgs,
    GpuCullRecord, GpuVertex, InlineModelGpu, InlineModelInstance, Renderer, WorldBatch, WorldGpu,
};

/// True only for an authoritative snapshot that explicitly omitted this
/// particular renderer instance. Never infer absence from a missing initial
/// pose: `uploaded == None` still contains the compiled model geometry.
#[inline]
pub(in crate::renderer) fn hidden_inline_batch(batch: &WorldBatch, models: &[InlineModelGpu]) -> bool {
    if batch.inline_model_slot == u32::MAX { return false; }
    models.get(batch.inline_model_slot as usize)
        .is_some_and(|model| matches!(model.uploaded, Some(None)))
}

/// TaystJK CG_AddPacketEntities(qtrue) selects *entity instances* by the
/// authoritative snapshot's isPortalEnt bit. No initial/hidden model can
/// enter the sky pass, even if the BSP itself has skyportal geometry.
#[inline]
pub(in crate::renderer) fn sky_portal_inline_eligible(
    pose: Option<Option<InlineModelInstance>>,
) -> bool {
    matches!(pose, Some(Some(instance)) if instance.is_portal_entity)
}

/// The server can change isPortalEnt independently of origin or angles.
/// Such a change is a draw-selection update, never a vertex upload.
#[inline]
fn same_inline_geometry(
    uploaded: Option<Option<InlineModelInstance>>,
    target: Option<InlineModelInstance>,
) -> bool {
    match (uploaded, target) {
        (Some(Some(previous)), Some(next)) =>
            previous.model == next.model
                && previous.origin == next.origin
                && previous.axis == next.axis,
        (Some(None), None) => true,
        _ => false,
    }
}

impl Renderer {
    /// Apply this frame's CG_Mover inline-model poses: rewrite each moved
    /// model's private vertex region and its batches' CPU and GPU cull bounds.
    /// Authoritative mover presence is an explicit cull-record flag in BOTH
    /// CPU and GPU paths. Absent models retain their last valid vertex data;
    /// reactivation uploads a newly posed copy without reallocating buffers.
    pub(in crate::renderer) fn update_inline_models(
        &mut self,
        instances: Option<&[InlineModelInstance]>,
    ) {
        let Some(world) = self.world.as_mut() else {
            return;
        };
        if self.inline_transform_mode == crate::ui::InlineTransformMode::Gpu
            && world.inline_transform.is_none() && !world.inline_models.is_empty() {
            world.inline_transform = Some(super::inline_transform::InlineTransformGpu::new(
                &self.device, &world.vertex_buffer, &world.inline_models,
            ));
        }
        let WorldGpu {
            inline_models,
            inline_transform,
            coarse_batches,
            full_batches,
            auto4_batches,
            auto4_inline_start,
            auto4_inline_full_start,
            vertex_buffer,
            cull_records_buffer,
            raster_glass_dynamic,
            raster_glass_static,
            dynamic_lights,
            local_shadows,
            ..
        } = world;
        let stride = std::mem::size_of::<GpuVertex>() as u64;
        let record_stride = std::mem::size_of::<GpuCullRecord>() as u64;
        // Only GPU culling (and its debug view) reads cull-record bounds. Moving
        // fish otherwise cost one tiny write_buffer per stage per frame (~350 on
        // costadelsol, 0.3 ms). CPU culling reads `batch.bounds_*` instead, so
        // defer the records and rewrite them all once when GPU culling returns.
        let cull_records_read =
            self.gpu_driven_enabled || self.cull_debug_mode != crate::renderer::CullDebugMode::Off;
        if cull_records_read && std::mem::take(&mut self.inline_cull_bounds_stale) {
            for model in inline_models.iter() {
                for (set, indices) in [
                    (&*coarse_batches, &model.coarse_batches),
                    (&*full_batches, &model.full_batches),
                ] {
                    for &index in indices {
                        let Some(batch) = set.get(index) else { continue; };
                        write_cull_bounds(&self.queue, cull_records_buffer, record_stride, batch);
                    }
                }
            }
        }
        let poses = &mut self.inline_pose_scratch;
        collect_inline_poses(
            poses,
            inline_models.iter().map(|model| model.model),
            instances,
        );
        let mut glass_poses_changed = false;
        for (slot, model) in inline_models.iter_mut().enumerate() {
            let target = poses.get_mut(model.model as usize).and_then(Vec::pop);
            // Portal membership can change without a pose change. Retain its
            // current per-instance flag, but avoid a needless vertex upload or
            // shadow invalidation when only isPortalEnt changed.
            if same_inline_geometry(model.uploaded, target) {
                model.uploaded = Some(target);
                continue;
            }
            let was_hidden = matches!(model.uploaded, Some(None));
            model.uploaded = Some(target);
            glass_poses_changed |= model.rt_mask_sources.iter().any(|source| source.material.glass);
            // Mark the individual instance invisible in the EXISTING cull
            // record (draw.w bit 1). GPU culling and its early Hi-Z phase
            // both reject it, including compacted indirect draws. Never
            // relocate or collapse its vertices just to hide the model.
            let hidden = target.is_none();
            // Preserve material flags when an inline mover changes presence.
            const CULL_DRAW_FLAGS_OFFSET: u64 = std::mem::offset_of!(GpuCullRecord, draw) as u64
                + 3 * std::mem::size_of::<u32>() as u64;
            // Moving fish update their pose every frame. Do not issue a tiny
            // queue.write_buffer for every stage every frame unless presence
            // actually changes (initial records are already marked inline).
            if was_hidden != hidden {
                for slot in 0..local_shadows.cache_entries.len() {
                    if local_shadows.cache_entries[slot].light_index.is_some()
                        && !local_shadows.pending_slots.contains(&slot) {
                        local_shadows.pending_slots.push(slot);
                    }
                }
                for (set, indices) in [
                    (&mut *coarse_batches, &model.coarse_batches),
                    (&mut *full_batches, &model.full_batches),
                ] {
                    for &index in indices {
                        let Some(batch) = set.get(index) else { continue; };
                        let flags: u32 = 1 | (u32::from(hidden) << 1)
                            | (u32::from(batch.source.pipeline.class == DrawClass::Transparent) << 3)
                            | (world_blend_probe_bucket(batch.source.pipeline) << 5)
                            | (u32::from(batch.source.water) << 8);
                        self.queue.write_buffer(
                            cull_records_buffer,
                            u64::from(batch.cull_index) * record_stride + CULL_DRAW_FLAGS_OFFSET,
                            bytemuck::bytes_of(&flags),
                        );
                    }
                }
            }
            let Some(pose) = target else {
                if let Some(resource) = inline_transform.as_mut()
                    .and_then(|gpu| gpu.models.get_mut(slot)).and_then(Option::as_mut) {
                    resource.cancel();
                }
                // No geometry upload, CPU bounds mutation or collapsed triangles.
                // Preserve compiled/last-active vertices for instant reappearance.
                continue;
            };
            let gpu_model = inline_transform.as_mut()
                .filter(|_| self.inline_transform_mode == crate::ui::InlineTransformMode::Gpu)
                .and_then(|gpu| gpu.models.get_mut(slot)).and_then(Option::as_mut);
            let (minimum, maximum) = if let Some(gpu_model) = gpu_model {
                gpu_model.update(&self.queue, pose)
            } else if let Some(pool) = self.inline_transform_workers.as_ref()
                .filter(|_| self.inline_transform_mode == crate::ui::InlineTransformMode::CpuWorkers) {
                let bounds = super::geometry::posed_inline_vertices_workers(
                    &model.base, pose, &mut self.inline_vertex_scratch, pool,
                );
                self.queue.write_buffer(
                    vertex_buffer,
                    u64::from(model.gpu_vertex_start) * stride,
                    bytemuck::cast_slice(&self.inline_vertex_scratch),
                );
                bounds
            } else {
                let (vertices, minimum, maximum) = posed_inline_vertices(&model.base, pose);
                self.queue.write_buffer(
                    vertex_buffer,
                    u64::from(model.gpu_vertex_start) * stride,
                    bytemuck::cast_slice(&vertices),
                );
                (minimum, maximum)
            };
            for (set, indices) in [
                (&mut *coarse_batches, &model.coarse_batches),
                (&mut *full_batches, &model.full_batches),
            ] {
                for &index in indices {
                    let Some(batch) = set.get_mut(index) else { continue; };
                    batch.bounds_min = minimum;
                    batch.bounds_max = maximum;
                    if cull_records_read {
                        write_cull_bounds(&self.queue, cull_records_buffer, record_stride, batch);
                    } else {
                        self.inline_cull_bounds_stale = true;
                    }
                }
            }
            // AUTO 4 is a shallow alias of FULL: the GPU cull-record flag
            // and bounds are shared; only the CPU batch bounds need syncing.
            for &full_index in &model.full_batches {
                let Some(relative) = full_index.checked_sub(*auto4_inline_full_start) else { continue; };
                let auto4_index = (*auto4_inline_start).saturating_add(relative);
                if let Some(batch) = auto4_batches.get_mut(auto4_index) {
                    batch.bounds_min = minimum;
                    batch.bounds_max = maximum;
                }
            }
        }
        if glass_poses_changed {
            raster_glass_dynamic.clear();
            for model in inline_models.iter() {
                let Some(Some(pose)) = model.uploaded else { continue; };
                raster_glass_dynamic.extend(model.raster_glass_local.iter().copied().map(|tri| {
                    crate::renderer::lighting::glass_transmission::transformed_triangle(tri, pose)
                }));
            }
            // Update the directional caster list whenever glass movers change.
            // Static BSP glass is otherwise uploaded once on map load.
            let count = crate::renderer::lighting::glass_transmission::upload_raster_sun_glass(
                &self.queue, &local_shadows.glass_buffer, raster_glass_static, raster_glass_dynamic);
            if count > crate::renderer::lighting::glass_transmission::RASTER_SUN_GLASS_SLOTS
                * crate::renderer::lighting::glass_transmission::RASTER_GLASS_PER_LIGHT {
                // Overflow is conservatively disabled by upload_raster_sun_glass.
                // Do not emit a warning each frame for moving glass.
            }
            // Mover transforms invalidate only the RGB caster list, not the
            // static opaque depth cubemap. Avoid six shadow redraws each frame.
            if self.tinted_glass_enabled {
                for (slot, entry) in local_shadows.cache_entries.iter().enumerate() {
                    let Some(light_index) = entry.light_index else { continue; };
                    let Some(light) = dynamic_lights.get(light_index) else { continue; };
                    let (glass, _) = crate::renderer::lighting::glass_transmission::pack_raster_glass_slot(
                        light.position, light.radius, raster_glass_static, raster_glass_dynamic);
                    self.queue.write_buffer(&local_shadows.glass_buffer,
                        (slot * std::mem::size_of::<crate::renderer::lighting::glass_transmission::RasterGlassSlot>()) as u64,
                        bytemuck::bytes_of(&glass));
                }
            }
        }
    }
}

/// Upload a batch's CPU bounds into its GPU cull record (min/max, w padded).
fn write_cull_bounds(queue: &wgpu::Queue, records: &wgpu::Buffer, record_stride: u64, batch: &WorldBatch) {
    let (minimum, maximum) = (batch.bounds_min, batch.bounds_max);
    let bounds: [f32; 8] = [
        minimum[0], minimum[1], minimum[2], 0.0, maximum[0], maximum[1], maximum[2], 0.0,
    ];
    queue.write_buffer(records, u64::from(batch.cull_index) * record_stride, bytemuck::cast_slice(&bounds));
}

/// One pose per render slot, preserving every server submission for a shared
/// handle. Vectors retain their storage across frames.
fn collect_inline_poses(
    poses: &mut Vec<Vec<InlineModelInstance>>,
    models: impl Iterator<Item = u32> + Clone,
    instances: Option<&[InlineModelInstance]>,
) {
    for slot in poses.iter_mut() {
        slot.clear();
    }
    let slots = models
        .clone()
        .map(|model| model as usize + 1)
        .max()
        .unwrap_or(0);
    if poses.len() < slots {
        poses.resize_with(slots, Vec::new);
    }
    if let Some(list) = instances {
        // Pop in submission order without per-model cursors.
        for instance in list.iter().rev() {
            if let Some(slot) = poses.get_mut(instance.model as usize) {
                slot.push(*instance);
            }
        }
    } else {
        // A solo preview shows one compiled pose per model, not spare copies.
        for model in models {
            let slot = &mut poses[model as usize];
            if slot.is_empty() {
                slot.push(InlineModelInstance::compiled(model));
            }
        }
    }
}

#[cfg(test)]
mod hidden_state_tests {
    use super::*;

    #[test]
    fn authoritative_absence_is_distinct_from_the_uninitialized_pose() {
        let active = Some(Some(InlineModelInstance::compiled(9)));
        assert!(!matches!(None::<Option<InlineModelInstance>>, Some(None)));
        assert!(!matches!(active, Some(None)));
        assert!(matches!(Some(None::<InlineModelInstance>), Some(None)));
    }
}

#[cfg(test)]
mod instance_tests {
    use super::*;

    #[test]
    fn shared_bsp_handles_keep_both_poses_and_hide_removed_instances() {
        let mut first = InlineModelInstance::compiled(9);
        first.origin = [2109.0, -1837.0, -851.0];
        let mut second = first;
        second.origin = [49805.0, -41942.0, -4245.0];
        let mut poses = Vec::new();
        collect_inline_poses(&mut poses, [8, 9, 9].into_iter(), Some(&[first, second]));
        assert_eq!(poses[9].pop(), Some(first));
        assert_eq!(poses[9].pop(), Some(second));
        assert_eq!(poses[9].pop(), None);
        assert_eq!(poses[8].pop(), None);
        collect_inline_poses(&mut poses, [8, 9, 9].into_iter(), Some(&[second]));
        assert_eq!(poses[9].pop(), Some(second));
        assert_eq!(poses[9].pop(), None);
        collect_inline_poses(&mut poses, [8, 9, 9].into_iter(), None);
        assert_eq!(poses[9].pop(), Some(InlineModelInstance::compiled(9)));
        assert_eq!(poses[9].pop(), None);
        collect_inline_poses(&mut poses, [8, 9, 9].into_iter(), Some(&[]));
        assert!(poses.iter().all(Vec::is_empty));
    }
}

/// Promoted water draws the shared camera-centred ocean clipmap in place of its
/// authored brush face. The clipmap is one mesh for the whole world, so the
/// range comes from the renderer rather than from per-batch state.
/// Legacy fog redraws follow each surface's final stage.
pub(in crate::renderer) fn inline_batch_needs_fog(
    fog: &weather::FogSystem,
    batch: &WorldBatch,
) -> bool {
    fog.legacy_uses_separate_pass(
        batch.source.fog_is_global,
        batch.source.legacy2_fog_in_stage_safe,
        batch.source.global_fog_post_eligible,
    ) && (batch.source.fog[3] > 0.001 || fog.legacy_drawfog_value() == 1)
}

/// Per visible mover batch: whether it continues the previous batch's surface,
/// and its fog kind. Bit 0 of the kind marks the surface's last visible stage,
/// bit 1 (last stages only) that some stage of the surface writes depth. These
/// pick the legacy fog redraw, so they belong to the run class.
pub(in crate::renderer) fn inline_stage_info(
    batches: &[WorldBatch],
    visible: &[usize],
) -> (Vec<bool>, Vec<u8>) {
    let continues: Vec<bool> = visible
        .iter()
        .enumerate()
        .map(|(position, &index)| {
            position > 0 && same_world_surface(&batches[index], &batches[visible[position - 1]])
        })
        .collect();
    let kinds = (0..visible.len())
        .map(|position| {
            if continues.get(position + 1).copied().unwrap_or(false) {
                return 0;
            }
            let mut writes_depth = false;
            let mut member = position;
            loop {
                writes_depth |= batches[visible[member]].source.pipeline.depth_write;
                if !continues[member] {
                    break;
                }
                member -= 1;
            }
            1 | (u8::from(writes_depth) << 1)
        })
        .collect();
    (continues, kinds)
}

/// `Some(surface_writes_depth)` when the run led by `batch` (fog kind `kind`)
/// needs a legacy fog redraw after it.
pub(in crate::renderer) fn inline_run_fog_depth(
    fog: &weather::FogSystem,
    ocean_enabled: bool,
    batch: &WorldBatch,
    kind: u8,
) -> Option<bool> {
    (kind & 1 != 0
        && batch.source.pipeline.class != DrawClass::Sky
        // GodotOcean fogs itself (bsp.wgsl apply_ocean_legacy_fog).
        && !(ocean_enabled && batch.ocean_clipmap.is_some())
        && inline_batch_needs_fog(fog, batch))
    .then_some(kind & 2 != 0)
}

/// Retained scratch for `plan_inline_runs` / `pack_inline_draws`.
#[derive(Default)]
pub(in crate::renderer) struct InlineDrawScratch {
    pub(in crate::renderer) group_offsets: Vec<u32>,
    pub(in crate::renderer) group_order: Vec<u32>,
    pub(in crate::renderer) done: Vec<bool>,
    /// Visible positions in draw order.
    pub(in crate::renderer) order: Vec<u32>,
    /// `(first index into order, count)` of each multi-draw.
    pub(in crate::renderer) runs: Vec<(u32, u32)>,
    /// State class of each run.
    pub(in crate::renderer) run_class: Vec<u32>,
    pub(in crate::renderer) args: Vec<DrawIndexedIndirectArgs>,
}

/// Orders the visible mover batches into runs that each share one draw state.
///
/// `group_of[p]` is the state class of visible batch `p` and `continues[p]` is
/// true when `p` is a later stage of the same surface as `p - 1`. A run is
/// emitted at its first member and pulls in every later member of the class,
/// except a member whose previous stage has not been drawn yet: it waits for a
/// later run. That keeps every surface's stages in authored order (an opaque
/// base must precede the depth-equal stage layered on it) while still merging
/// the same stage of different surfaces, like OpenJK's per-shader batches.
pub(in crate::renderer) fn plan_inline_runs(
    group_of: &[u32],
    continues: &[bool],
    group_count: usize,
    scratch: &mut InlineDrawScratch,
) {
    let n = group_of.len();
    scratch.group_offsets.clear();
    scratch.group_offsets.resize(group_count + 1, 0);
    for &group in group_of {
        scratch.group_offsets[group as usize + 1] += 1;
    }
    for group in 0..group_count {
        scratch.group_offsets[group + 1] += scratch.group_offsets[group];
    }
    scratch.group_order.clear();
    scratch.group_order.resize(n, 0);
    // Counting sort by class; positions stay ascending inside a class.
    let mut cursor = scratch.group_offsets.clone();
    for (position, &group) in group_of.iter().enumerate() {
        scratch.group_order[cursor[group as usize] as usize] = position as u32;
        cursor[group as usize] += 1;
    }
    scratch.done.clear();
    scratch.done.resize(n, false);
    scratch.order.clear();
    scratch.runs.clear();
    scratch.run_class.clear();
    for position in 0..n {
        if scratch.done[position] {
            continue;
        }
        let group = group_of[position] as usize;
        let first = scratch.order.len() as u32;
        let members =
            scratch.group_offsets[group] as usize..scratch.group_offsets[group + 1] as usize;
        for slot in members {
            let member = scratch.group_order[slot] as usize;
            if scratch.done[member] || (continues[member] && !scratch.done[member - 1]) {
                continue;
            }
            scratch.done[member] = true;
            scratch.order.push(member as u32);
        }
        scratch
            .runs
            .push((first, scratch.order.len() as u32 - first));
        scratch.run_class.push(group as u32);
    }
}

/// Plans and packs the visible mover batches: the indirect arguments of every
/// run are contiguous in `buffer` from `region_base`, in run order. Returns
/// false when the region is too small (callers then draw batch by batch).
#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn pack_inline_draws(
    queue: &wgpu::Queue,
    batches: &[WorldBatch],
    visible: &[usize],
    scratch: &mut InlineDrawScratch,
    group_count: usize,
    buffer: &wgpu::Buffer,
    region_base: u32,
    region_capacity: u32,
    secondary: bool,
) -> bool {
    if visible.len() > region_capacity as usize {
        return false;
    }
    // The class also carries the fog kind (see `inline_stage_info`) so a run is
    // fog-homogeneous: its fog redraw is one more multi-draw over the same range.
    let (continues, kinds) = inline_stage_info(batches, visible);
    let group_of: Vec<u32> = visible
        .iter()
        .zip(&kinds)
        .map(|(&index, &kind)| batches[index].inline_group * 4 + u32::from(kind))
        .collect();
    plan_inline_runs(&group_of, &continues, group_count * 4, scratch);
    let mut args = std::mem::take(&mut scratch.args);
    args.clear();
    args.extend(scratch.order.iter().map(|&position| {
        let batch = &batches[visible[position as usize]];
        let range=if secondary {crate::renderer::secondary_batch_range(batch)} else {&batch.indexed_range};
        DrawIndexedIndirectArgs {
            index_count: range.end-range.start,
            instance_count: 1,
            first_index: range.start,
            base_vertex: 0,
            first_instance: 0,
        }
    }));
    if !args.is_empty() {
        queue.write_buffer(
            buffer,
            u64::from(region_base) * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
            bytemuck::cast_slice(&args),
        );
    }
    scratch.args = args;
    true
}

#[cfg(test)]
mod portal_entity_tests {
    use super::*;

    #[test]
    fn portal_membership_is_per_instance_not_per_bsp_handle() {
        let mut portal_mover = InlineModelInstance::compiled(12);
        portal_mover.is_portal_entity = true;
        let ordinary_mover = InlineModelInstance::compiled(12);
        assert!(sky_portal_inline_eligible(Some(Some(portal_mover))));
        assert!(!sky_portal_inline_eligible(Some(Some(ordinary_mover))));
        assert!(!sky_portal_inline_eligible(Some(None)));
        assert!(!sky_portal_inline_eligible(None));
    }

    #[test]
    fn portal_flag_only_transition_does_not_change_uploaded_geometry() {
        let old = InlineModelInstance::compiled(12);
        let mut marked = old;
        marked.is_portal_entity = true;
        assert!(same_inline_geometry(Some(Some(old)), Some(marked)));
        assert!(same_inline_geometry(Some(Some(marked)), Some(old)));
        assert!(!same_inline_geometry(None, Some(marked)));
        marked.origin[0] = 128.0;
        assert!(!same_inline_geometry(Some(Some(old)), Some(marked)));
        assert!(!same_inline_geometry(Some(Some(old)), None));
    }
}
