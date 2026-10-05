//! World inline models.
use crate::renderer::{
    posed_inline_vertices, same_world_surface, weather, DrawClass, DrawIndexedIndirectArgs,
    GpuCullRecord, GpuVertex, InlineModelInstance, Renderer, WorldBatch, WorldGpu,
};

impl Renderer {
    /// Apply this frame's CG_Mover inline-model poses: rewrite each moved
    /// model's private vertex region and its batches' CPU and GPU cull bounds.
    /// Models absent from an authoritative list collapse to zero-area
    /// triangles, so no draw list has to change.
    pub(in crate::renderer) fn update_inline_models(
        &mut self,
        instances: Option<&[InlineModelInstance]>,
    ) {
        let Some(world) = self.world.as_mut() else {
            return;
        };
        let WorldGpu {
            inline_models,
            coarse_batches,
            full_batches,
            auto4_batches,
            auto4_inline_start,
            auto4_inline_full_start,
            vertex_buffer,
            cull_records_buffer,
            ..
        } = world;
        let stride = std::mem::size_of::<GpuVertex>() as u64;
        let record_stride = std::mem::size_of::<GpuCullRecord>() as u64;
        // Index the snapshot by model number once (first instance wins, as the
        // previous linear search did) instead of scanning it for every model.
        let poses = &mut self.inline_pose_scratch;
        poses.clear();
        if let Some(list) = instances {
            let slots = inline_models
                .iter()
                .map(|model| model.model as usize + 1)
                .max()
                .unwrap_or(0);
            poses.resize(slots, None);
            for instance in list {
                if let Some(slot) = poses.get_mut(instance.model as usize) {
                    slot.get_or_insert(*instance);
                }
            }
        }
        for model in inline_models.iter_mut() {
            let target = match instances {
                None => Some(InlineModelInstance::compiled(model.model)),
                Some(_) => poses.get(model.model as usize).copied().flatten(),
            };
            if model.uploaded == Some(target) {
                continue;
            }
            model.uploaded = Some(target);
            let (vertices, minimum, maximum) = posed_inline_vertices(&model.base, target);
            self.queue.write_buffer(
                vertex_buffer,
                u64::from(model.gpu_vertex_start) * stride,
                bytemuck::cast_slice(&vertices),
            );
            let bounds: [f32; 8] = [
                minimum[0], minimum[1], minimum[2], 0.0, maximum[0], maximum[1], maximum[2], 0.0,
            ];
            for (set, indices) in [
                (&mut *coarse_batches, &model.coarse_batches),
                (&mut *full_batches, &model.full_batches),
            ] {
                for &index in indices {
                    let Some(batch) = set.get_mut(index) else {
                        continue;
                    };
                    batch.bounds_min = minimum;
                    batch.bounds_max = maximum;
                    self.queue.write_buffer(
                        cull_records_buffer,
                        u64::from(batch.cull_index) * record_stride,
                        bytemuck::cast_slice(&bounds),
                    );
                }
            }
            // AUTO 4 keeps static portal-plan geometry first and appends one
            // shallow copy of the FULL inline-model tail. The cull record itself
            // is shared with FULL; keep only the CPU bounds alias in sync here.
            for &full_index in &model.full_batches {
                let Some(relative) = full_index.checked_sub(*auto4_inline_full_start) else {
                    continue;
                };
                let auto4_index = (*auto4_inline_start).saturating_add(relative);
                if let Some(batch) = auto4_batches.get_mut(auto4_index) {
                    batch.bounds_min = minimum;
                    batch.bounds_max = maximum;
                }
            }
        }
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
        DrawIndexedIndirectArgs {
            index_count: batch
                .indexed_range
                .end
                .saturating_sub(batch.indexed_range.start),
            instance_count: 1,
            first_index: batch.indexed_range.start,
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
