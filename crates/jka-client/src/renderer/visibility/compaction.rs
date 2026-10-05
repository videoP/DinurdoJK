//! Visibility compaction.
use crate::renderer::{
    gpu_vertex_key, mpsc, thread, Arc, AtomicBool, BTreeMap, DrawBatch, DrawClass,
    DrawIndexedIndirectArgs, GpuCullRecord, GpuVertex, JoinHandle, Ordering,
    PreparedPortalDrawPlan, PreparedPortalPlanBatchRef, Receiver, WorldBatch, WorldGpu,
    AUTO4_COLLAPSE_QUEUE_DEPTH,
};

/// Concatenate one AUTO 4 geometry's FULL piece index ranges.
#[derive(Debug)]
pub(in crate::renderer) struct Auto4CollapseRequest {
    pub(in crate::renderer) geometry: usize,
    pub(in crate::renderer) members: Vec<usize>,
}

#[derive(Debug)]
pub(in crate::renderer) struct Auto4CollapseResult {
    pub(in crate::renderer) geometry: usize,
    pub(in crate::renderer) indices: Vec<u32>,
}

pub(in crate::renderer) struct Auto4CollapseWorker {
    pub(in crate::renderer) sender: Option<mpsc::SyncSender<Auto4CollapseRequest>>,
    pub(in crate::renderer) receiver: Receiver<Auto4CollapseResult>,
    pub(in crate::renderer) cancel: Arc<AtomicBool>,
    pub(in crate::renderer) thread: Option<JoinHandle<()>>,
}

impl Auto4CollapseWorker {
    pub(in crate::renderer) fn spawn(
        source_indices: Arc<Vec<u32>>,
        piece_ranges: Arc<Vec<std::ops::Range<u32>>>,
    ) -> Result<Self, String> {
        let (request_tx, request_rx) =
            mpsc::sync_channel::<Auto4CollapseRequest>(AUTO4_COLLAPSE_QUEUE_DEPTH);
        let (result_tx, result_rx) = mpsc::channel::<Auto4CollapseResult>();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let thread = thread::Builder::new()
            .name("jka-auto4-collapse".to_owned())
            .spawn(move || {
                while !worker_cancel.load(Ordering::Acquire) {
                    let Ok(request) = request_rx.recv() else {
                        break;
                    };
                    if worker_cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let index_count = request
                        .members
                        .iter()
                        .filter_map(|&member| piece_ranges.get(member))
                        .map(|range| range.end.saturating_sub(range.start) as usize)
                        .sum::<usize>();
                    let mut indices = Vec::with_capacity(index_count);
                    for &member in &request.members {
                        let Some(range) = piece_ranges.get(member) else {
                            continue;
                        };
                        if let Some(slice) =
                            source_indices.get(range.start as usize..range.end as usize)
                        {
                            indices.extend_from_slice(slice);
                        }
                    }
                    if result_tx
                        .send(Auto4CollapseResult {
                            geometry: request.geometry,
                            indices,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| format!("could not start AUTO 4 collapse worker: {error}"))?;
        Ok(Self {
            sender: Some(request_tx),
            receiver: result_rx,
            cancel,
            thread: Some(thread),
        })
    }
}

impl Drop for Auto4CollapseWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Residency of one AUTO 4 physical geometry (shared by every stage recipe
/// that draws the same FULL piece ranges).
#[derive(Debug)]
pub(in crate::renderer) enum Auto4GeometryState {
    /// FULL's existing index data already holds this recipe as one run.
    Static,
    Uncached,
    Pending,
    /// Could not fit while every resident geometry was protected by the
    /// current plan. Retry only after entering a different cluster to avoid
    /// rebuild churn on a plan whose working set exceeds the cache.
    Deferred,
    /// Larger than the entire cache; always drawn as its FULL pieces.
    Uncacheable,
    Ready {
        range: std::ops::Range<u32>,
        last_used_frame: u64,
    },
}

/// Per-plan submission statistics, logged when `r_cullDebug` is active.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::renderer) struct Auto4PlanStats {
    pub(in crate::renderer) physical_draws: usize,
    pub(in crate::renderer) fallback_recipes: usize,
    pub(in crate::renderer) fallback_draws: usize,
    pub(in crate::renderer) area_split_recipes: usize,
    pub(in crate::renderer) base_draws: usize,
}

pub(in crate::renderer) struct Auto4CollapseCache {
    pub(in crate::renderer) worker: Option<Auto4CollapseWorker>,
    pub(in crate::renderer) states: Vec<Auto4GeometryState>,
    pub(in crate::renderer) capacity_indices: u32,
    pub(in crate::renderer) free_ranges: Vec<std::ops::Range<u32>>,
    pub(in crate::renderer) used_indices: u32,
    pub(in crate::renderer) frame: u64,
    pub(in crate::renderer) queued: u64,
    pub(in crate::renderer) completed: u64,
    pub(in crate::renderer) evictions: u64,
    pub(in crate::renderer) oversize: u64,
    pub(in crate::renderer) current_cluster: Option<usize>,
    pub(in crate::renderer) current_area_mask: Option<[u8; 32]>,
    /// Geometries referenced by the current plan; reused scratch so eviction
    /// never allocates.
    pub(in crate::renderer) protected: Vec<bool>,
    pub(in crate::renderer) stats: Auto4PlanStats,
}

impl Auto4CollapseCache {
    pub(in crate::renderer) fn new(states: Vec<Auto4GeometryState>) -> Self {
        let geometry_count = states.len();
        Self {
            worker: None,
            states,
            capacity_indices: 0,
            free_ranges: Vec::new(),
            used_indices: 0,
            frame: 0,
            queued: 0,
            completed: 0,
            evictions: 0,
            oversize: 0,
            current_cluster: None,
            current_area_mask: None,
            protected: vec![false; geometry_count],
            stats: Auto4PlanStats::default(),
        }
    }

    pub(in crate::renderer) fn allocate(&mut self, count: u32) -> Option<std::ops::Range<u32>> {
        let slot = self
            .free_ranges
            .iter()
            .position(|range| range.end.saturating_sub(range.start) >= count)?;
        let start = self.free_ranges[slot].start;
        let end = start.checked_add(count)?;
        if end == self.free_ranges[slot].end {
            self.free_ranges.remove(slot);
        } else {
            self.free_ranges[slot].start = end;
        }
        self.used_indices = self.used_indices.saturating_add(count);
        Some(start..end)
    }

    pub(in crate::renderer) fn release(&mut self, range: std::ops::Range<u32>) {
        self.used_indices = self
            .used_indices
            .saturating_sub(range.end.saturating_sub(range.start));
        self.free_ranges.push(range);
        self.free_ranges.sort_unstable_by_key(|range| range.start);
        let mut merged = Vec::<std::ops::Range<u32>>::with_capacity(self.free_ranges.len());
        for range in self.free_ranges.drain(..) {
            if let Some(last) = merged.last_mut() {
                if last.end >= range.start {
                    last.end = last.end.max(range.end);
                    continue;
                }
            }
            merged.push(range);
        }
        self.free_ranges = merged;
    }
}

pub(in crate::renderer) struct IndexedGeometryBuilder<'a> {
    pub(in crate::renderer) source: &'a [GpuVertex],
    pub(in crate::renderer) source_triangle_surfaces: &'a [u32],
    pub(in crate::renderer) vertices: Vec<GpuVertex>,
    pub(in crate::renderer) legacy_dlight_surface_ids: Vec<u32>,
    // A shared geometric vertex may belong to different authored BSP surfaces.
    // Keep those identities distinct so the Legacy-only flat surface-id stream
    // remains exact without enlarging GpuVertex for every renderer mode.
    // Open-addressing table of indices into `vertices` (`u32::MAX` = empty). A
    // slot is matched by comparing the stored vertex's key and surface id, so no
    // 64-byte keys are stored: the table is a few MiB instead of ~100 MiB of
    // scattered map entries. Indices are assigned in first-seen order, so the
    // lookup structure cannot change the result.
    pub(in crate::renderer) table: Vec<u32>,
    pub(in crate::renderer) table_mask: usize,
    pub(in crate::renderer) indices: Vec<u32>,
    pub(in crate::renderer) cache: BTreeMap<(u32, u32, u32), std::ops::Range<u32>>,
    pub(in crate::renderer) source_to_index: Vec<Option<u32>>,
}

impl<'a> IndexedGeometryBuilder<'a> {
    pub(in crate::renderer) fn new(
        source: &'a [GpuVertex],
        source_triangle_surfaces: &'a [u32],
    ) -> Self {
        // At most one entry per source vertex, so this keeps the load under 2/3.
        let slots = (source.len() * 3 / 2).max(16).next_power_of_two();
        Self {
            source,
            source_triangle_surfaces,
            vertices: Vec::new(),
            legacy_dlight_surface_ids: Vec::new(),
            table: vec![u32::MAX; slots],
            table_mask: slots - 1,
            indices: Vec::new(),
            cache: BTreeMap::new(),
            source_to_index: vec![None; source.len()],
        }
    }

    pub(in crate::renderer) fn intern(&mut self, vertex: GpuVertex, surface_id: u32) -> u32 {
        use std::hash::Hasher;
        let key = gpu_vertex_key(&vertex);
        let mut words = [0_u32; 16];
        words[..15].copy_from_slice(&key);
        words[15] = surface_id;
        let mut hasher = crate::scene::FoldHasher::default();
        hasher.write(bytemuck::cast_slice(&words));
        let mut slot = hasher.finish() as usize & self.table_mask;
        loop {
            let existing = self.table[slot];
            if existing == u32::MAX {
                break;
            }
            let candidate = existing as usize;
            if self.legacy_dlight_surface_ids[candidate] == surface_id
                && gpu_vertex_key(&self.vertices[candidate]) == key
            {
                return existing;
            }
            slot = (slot + 1) & self.table_mask;
        }
        let index = u32::try_from(self.vertices.len()).unwrap_or(u32::MAX);
        self.vertices.push(vertex);
        self.legacy_dlight_surface_ids.push(surface_id);
        self.table[slot] = index;
        index
    }

    pub(in crate::renderer) fn append_indices(&mut self, values: &[u32]) -> std::ops::Range<u32> {
        let start = u32::try_from(self.indices.len()).unwrap_or(u32::MAX);
        self.indices.extend_from_slice(values);
        let end = u32::try_from(self.indices.len()).unwrap_or(u32::MAX);
        start..end
    }

    /// Appends the ocean clipmap as its own vertex/index block. Its vertices are
    /// grid records rather than world positions, so they must not go through
    /// `intern` and be merged with real BSP geometry.
    pub(in crate::renderer) fn append_clipmap(
        &mut self,
        vertices: &[GpuVertex],
        indices: &[u32],
    ) -> std::ops::Range<u32> {
        let vertex_base = u32::try_from(self.vertices.len()).unwrap_or(u32::MAX);
        self.vertices.extend_from_slice(vertices);
        self.legacy_dlight_surface_ids
            .extend(std::iter::repeat(u32::MAX).take(vertices.len()));
        let start = u32::try_from(self.indices.len()).unwrap_or(u32::MAX);
        self.indices.extend(
            indices
                .iter()
                .map(|index| vertex_base.saturating_add(*index)),
        );
        let end = u32::try_from(self.indices.len()).unwrap_or(u32::MAX);
        start..end
    }

    pub(in crate::renderer) fn batch_range(&mut self, batch: &DrawBatch) -> std::ops::Range<u32> {
        let key = (batch.vertices.start, batch.vertices.end, 0u32);
        if let Some(range) = self.cache.get(&key) {
            return range.clone();
        }

        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        let source = self.source;
        let mut values = Vec::with_capacity(end.saturating_sub(start));
        for source_index in start..end {
            // FULL pieces re-cover the coarse batches' source vertices. A source
            // index always interns to the same result, so repeat visits reuse it
            // instead of hashing the vertex again.
            let known = self.source_to_index.get(source_index).copied().flatten();
            let index = match known {
                Some(index) => index,
                None => {
                    let surface_id = self
                        .source_triangle_surfaces
                        .get(source_index / 3)
                        .copied()
                        .unwrap_or(u32::MAX);
                    let index = self.intern(source[source_index], surface_id);
                    if let Some(slot) = self.source_to_index.get_mut(source_index) {
                        *slot = Some(index);
                    }
                    index
                }
            };
            values.push(index);
        }
        let range = self.append_indices(&values);
        self.cache.insert(key, range.clone());
        range
    }
}

pub(in crate::renderer) struct Auto4Instance {
    pub(in crate::renderer) batches: Vec<WorldBatch>,
    pub(in crate::renderer) variant_base: usize,
    pub(in crate::renderer) always_start: usize,
    pub(in crate::renderer) variant_members: Vec<Vec<usize>>,
    pub(in crate::renderer) variant_geometry: Vec<usize>,
    pub(in crate::renderer) variant_area_mixed: Vec<bool>,
    pub(in crate::renderer) geometry_members: Vec<Vec<usize>>,
    pub(in crate::renderer) geometry_variants: Vec<Vec<usize>>,
    pub(in crate::renderer) geometry_states: Vec<Auto4GeometryState>,
}

/// Instantiate AUTO 4's map-worker recipes against the uploaded FULL pieces.
/// FULL batches are aliased as-is (they are the exact cold fallback); every
/// recipe gets one batch with FULL's bind state and its own cull record.
/// Recipes whose pieces are already one run in FULL's index data draw from
/// that run immediately and never touch the collapse cache.
pub(in crate::renderer) fn instantiate_portal_draw_plans(
    plan: &PreparedPortalDrawPlan,
    full_batches: &[WorldBatch],
    inline_full_start: usize,
    cull_records: &mut Vec<GpuCullRecord>,
    initial_indirect: &mut Vec<DrawIndexedIndirectArgs>,
) -> Option<Auto4Instance> {
    if plan.plan_by_cluster.is_empty() || full_batches.is_empty() {
        return None;
    }
    let piece_count = plan.piece_count.min(inline_full_start);
    let world_piece = |member: usize| member < piece_count;

    let geometry_ranges = plan
        .geometries
        .iter()
        .map(|geometry| {
            let mut run: Option<std::ops::Range<u32>> = None;
            for &member in &geometry.members {
                if !world_piece(member) {
                    return None;
                }
                let range = &full_batches[member].indexed_range;
                match &mut run {
                    None => run = Some(range.clone()),
                    Some(run) if run.end == range.start => run.end = range.end,
                    Some(_) => return None,
                }
            }
            run
        })
        .collect::<Vec<_>>();
    let geometry_states = geometry_ranges
        .iter()
        .map(|range| {
            if range.is_some() {
                Auto4GeometryState::Static
            } else {
                Auto4GeometryState::Uncached
            }
        })
        .collect::<Vec<_>>();

    let mut batches = full_batches.to_vec();
    let variant_base = batches.len();
    let mut geometry_variants = vec![Vec::new(); plan.geometries.len()];
    for (index, variant) in plan.variants.iter().enumerate() {
        geometry_variants[variant.geometry].push(index);
        let representative = if world_piece(variant.representative) {
            variant.representative
        } else {
            0
        };
        let mut batch = full_batches[representative].clone();
        batch.indexed_range = geometry_ranges[variant.geometry].clone().unwrap_or(0..0);
        batch.source.pvs_signature.clear();
        batch.source.area_signature = variant.area_signature;
        batch.is_inline_entity = false;
        batch.ocean_clipmap = None;
        batch.ocean_clipmap_id = u8::MAX;
        batch.compact_group = None;
        batch.bounds_min = variant.bounds_min;
        batch.bounds_max = variant.bounds_max;

        let index_count = batch
            .indexed_range
            .end
            .saturating_sub(batch.indexed_range.start);
        let cull_index = u32::try_from(cull_records.len()).unwrap_or(u32::MAX);
        cull_records.push(GpuCullRecord {
            minimum: [
                variant.bounds_min[0],
                variant.bounds_min[1],
                variant.bounds_min[2],
                0.0,
            ],
            maximum: [
                variant.bounds_max[0],
                variant.bounds_max[1],
                variant.bounds_max[2],
                0.0,
            ],
            draw: [
                index_count,
                batch.indexed_range.start,
                u32::from(batch.source.pipeline.class == DrawClass::Sky),
                0,
            ],
            compact: [u32::MAX, 0, 0, 0],
        });
        initial_indirect.push(DrawIndexedIndirectArgs {
            index_count,
            instance_count: 1,
            first_index: batch.indexed_range.start,
            base_vertex: 0,
            first_instance: 0,
        });
        batch.cull_index = cull_index;
        batches.push(batch);
    }

    Some(Auto4Instance {
        batches,
        variant_base,
        always_start: piece_count,
        variant_members: plan
            .variants
            .iter()
            .map(|variant| variant.members.clone())
            .collect(),
        variant_geometry: plan
            .variants
            .iter()
            .map(|variant| variant.geometry)
            .collect(),
        variant_area_mixed: plan
            .variants
            .iter()
            .map(|variant| variant.area_mixed)
            .collect(),
        geometry_members: plan
            .geometries
            .iter()
            .map(|geometry| geometry.members.clone())
            .collect(),
        geometry_variants,
        geometry_states,
    })
}

pub(in crate::renderer) fn auto4_upload_variant_record(
    queue: &wgpu::Queue,
    world: &WorldGpu,
    batch_index: usize,
) {
    let Some(batch) = world.auto4_batches.get(batch_index) else {
        return;
    };
    // Recipe batches are never compacted: each one is a single real draw.
    let record = GpuCullRecord {
        minimum: [
            batch.bounds_min[0],
            batch.bounds_min[1],
            batch.bounds_min[2],
            0.0,
        ],
        maximum: [
            batch.bounds_max[0],
            batch.bounds_max[1],
            batch.bounds_max[2],
            0.0,
        ],
        draw: [
            batch
                .indexed_range
                .end
                .saturating_sub(batch.indexed_range.start),
            batch.indexed_range.start,
            u32::from(batch.source.pipeline.class == DrawClass::Sky),
            0,
        ],
        compact: [u32::MAX, 0, 0, 0],
    };
    queue.write_buffer(
        &world.cull_records_buffer,
        u64::from(batch.cull_index) * std::mem::size_of::<GpuCullRecord>() as u64,
        bytemuck::bytes_of(&record),
    );
}

/// Point every recipe that draws `geometry` at `range` (empty = not resident).
pub(in crate::renderer) fn auto4_set_geometry_range(
    queue: &wgpu::Queue,
    world: &mut WorldGpu,
    geometry: usize,
    range: std::ops::Range<u32>,
) {
    let variants = std::mem::take(&mut world.auto4_geometry_variants[geometry]);
    for &variant in &variants {
        let batch_index = world.auto4_variant_base.saturating_add(variant);
        if let Some(batch) = world.auto4_batches.get_mut(batch_index) {
            batch.indexed_range = range.clone();
        }
        auto4_upload_variant_record(queue, world, batch_index);
    }
    world.auto4_geometry_variants[geometry] = variants;
}

/// Flag the geometries referenced by `plan_id` so eviction keeps them.
pub(in crate::renderer) fn auto4_mark_protected(world: &mut WorldGpu, plan_id: usize) {
    let cache = &mut world.auto4_collapse_cache;
    cache.protected.fill(false);
    let Some(refs) = world.auto4_plan_refs.get(plan_id) else {
        return;
    };
    for reference in refs {
        if let PreparedPortalPlanBatchRef::Merged(variant) = *reference {
            if let Some(&geometry) = world.auto4_variant_geometry.get(variant) {
                cache.protected[geometry] = true;
            }
        }
    }
}

pub(in crate::renderer) fn auto4_evict_lru_geometry(
    queue: &wgpu::Queue,
    world: &mut WorldGpu,
) -> bool {
    let cache = &world.auto4_collapse_cache;
    let victim = cache
        .states
        .iter()
        .enumerate()
        .filter_map(|(geometry, state)| match state {
            Auto4GeometryState::Ready {
                last_used_frame, ..
            } if !cache.protected[geometry] => Some((geometry, *last_used_frame)),
            _ => None,
        })
        .min_by_key(|(_, last_used_frame)| *last_used_frame)
        .map(|(geometry, _)| geometry);
    let Some(victim) = victim else {
        return false;
    };
    let old = std::mem::replace(
        &mut world.auto4_collapse_cache.states[victim],
        Auto4GeometryState::Uncached,
    );
    let Auto4GeometryState::Ready { range, .. } = old else {
        return false;
    };
    world.auto4_collapse_cache.release(range);
    world.auto4_collapse_cache.evictions = world.auto4_collapse_cache.evictions.saturating_add(1);
    auto4_set_geometry_range(queue, world, victim, 0..0);
    true
}

/// Upload one finished physical collapse and switch its recipes to it.
pub(in crate::renderer) fn auto4_accept_collapse(
    queue: &wgpu::Queue,
    world: &mut WorldGpu,
    result: Auto4CollapseResult,
    plan_id: usize,
) {
    let geometry = result.geometry;
    if !matches!(
        world.auto4_collapse_cache.states.get(geometry),
        Some(Auto4GeometryState::Pending)
    ) {
        return;
    }
    let count = u32::try_from(result.indices.len()).unwrap_or(u32::MAX);
    if count == 0 || count > world.auto4_collapse_cache.capacity_indices {
        if count != 0 {
            world.auto4_collapse_cache.oversize =
                world.auto4_collapse_cache.oversize.saturating_add(1);
        }
        world.auto4_collapse_cache.states[geometry] = Auto4GeometryState::Uncacheable;
        return;
    }
    let mut range = world.auto4_collapse_cache.allocate(count);
    if range.is_none() {
        auto4_mark_protected(world, plan_id);
        while range.is_none() {
            if !auto4_evict_lru_geometry(queue, world) {
                break;
            }
            range = world.auto4_collapse_cache.allocate(count);
        }
    }
    let Some(range) = range else {
        world.auto4_collapse_cache.states[geometry] = Auto4GeometryState::Deferred;
        return;
    };
    queue.write_buffer(
        &world.index_buffer,
        u64::from(range.start) * std::mem::size_of::<u32>() as u64,
        bytemuck::cast_slice(&result.indices),
    );
    world.auto4_collapse_cache.states[geometry] = Auto4GeometryState::Ready {
        range: range.clone(),
        last_used_frame: world.auto4_collapse_cache.frame,
    };
    world.auto4_collapse_cache.completed = world.auto4_collapse_cache.completed.saturating_add(1);
    auto4_set_geometry_range(queue, world, geometry, range);
}
