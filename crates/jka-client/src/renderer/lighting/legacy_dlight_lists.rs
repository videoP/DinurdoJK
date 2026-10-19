//! Legacy dlight lit-surface lists, after rd-vulkan's R_RecursiveLightNode and
//! RB_LightingPass: each light visits only the BSP surfaces inside its radius
//! (a static surface BVH stands in for the node walk), and the lit triangles are
//! redrawn into one per-frame index list sorted by material, one draw each.
use super::legacy_dlights::legacy_dlight_touches_surface;
use crate::renderer::world::patch_lod::WorldPatchLod;
use crate::renderer::{
    aabb_intersects_clip_frustum, legacy_dlight_batch_selection, legacy_dlight_receives, scene,
    Mat4, PvsMode, TransientLight, Vec3, WorldBatch, WorldGpu,
};
use std::ops::Range;
use std::sync::{Mutex, OnceLock, PoisonError};

const LEAF_SIZE: usize = 4;
const MIN_INDEX_CAPACITY: u64 = 64 * 1024;

#[derive(Clone, Copy)]
struct Node {
    minimum: [f32; 3],
    maximum: [f32; 3],
    /// Leaf: first item. Internal: left child (right child follows it).
    first: u32,
    /// Item count; zero for an internal node.
    count: u32,
}

#[derive(Clone, Copy)]
enum Piece {
    /// Full-detail indices; static geometry never changes them.
    Static { first: u32, count: u32 },
    /// A curved patch, redrawn from its current LOD selection.
    Patch { grid: u32 },
}

#[derive(Clone, Copy)]
struct Entry {
    batch: u32,
    piece: Piece,
}

/// Load-once lookup: surface BVH plus, per surface, its triangles in every
/// receiving coarse (material) batch.
pub(in crate::renderer) struct LegacyDlightIndex {
    nodes: Vec<Node>,
    items: Vec<u32>,
    entry_start: Vec<u32>,
    entries: Vec<Entry>,
    /// Coarse batch -> material order (pipeline, then batch); `u32::MAX` if it
    /// does not receive Legacy dlights.
    batch_rank: Vec<u32>,
    rank_batch: Vec<u32>,
}

impl LegacyDlightIndex {
    fn build(
        surfaces: &[scene::LegacyDlightSurface],
        batches: &[WorldBatch],
        patch_lod: &WorldPatchLod,
    ) -> Self {
        let (nodes, items) = build_bvh(surfaces);

        let mut ranked = batches
            .iter()
            .enumerate()
            .filter(|(_, batch)| !batch.is_inline_entity && legacy_dlight_receives(&batch.source))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        ranked.sort_by(|&a, &b| {
            batches[a].source.pipeline.cmp(&batches[b].source.pipeline).then(a.cmp(&b))
        });
        let mut batch_rank = vec![u32::MAX; batches.len()];
        for (rank, &batch) in ranked.iter().enumerate() {
            batch_rank[batch] = rank as u32;
        }

        let mut pairs = Vec::<(u32, Entry)>::new();
        for &index in &ranked {
            let batch = &batches[index];
            let batch_id = index as u32;
            let Some(geometry) = batch.patch_geometry else {
                for run in &batch.legacy_dlight_runs {
                    if run.surface_id != u32::MAX {
                        pairs.push((run.surface_id, Entry {
                            batch: batch_id,
                            piece: Piece::Static {
                                first: run.indexed_range.start,
                                count: run.indexed_range.end - run.indexed_range.start,
                            },
                        }));
                    }
                }
                continue;
            };
            // Split each run around the patches it covers: static parts keep
            // their full-detail indices, patches follow the live LOD.
            let spans = patch_lod.patch_spans(geometry);
            let mut last_grid = usize::MAX;
            for run in patch_lod.original_runs(geometry) {
                if run.surface_id == u32::MAX {
                    continue;
                }
                let range = run.indexed_range.clone();
                let mut cursor = range.start;
                let first = spans.partition_point(|span| span.0 + span.1 <= range.start);
                for &(start, count, grid) in &spans[first..] {
                    if start >= range.end {
                        break;
                    }
                    if cursor < start {
                        pairs.push((run.surface_id, Entry {
                            batch: batch_id,
                            piece: Piece::Static { first: cursor, count: start - cursor },
                        }));
                    }
                    if grid != last_grid {
                        pairs.push((run.surface_id, Entry {
                            batch: batch_id,
                            piece: Piece::Patch { grid: grid as u32 },
                        }));
                        last_grid = grid;
                    }
                    cursor = cursor.max(start + count);
                }
                if cursor < range.end {
                    pairs.push((run.surface_id, Entry {
                        batch: batch_id,
                        piece: Piece::Static { first: cursor, count: range.end - cursor },
                    }));
                }
            }
        }
        pairs.sort_by_key(|(surface, _)| *surface);
        let mut entry_start = vec![0u32; surfaces.len() + 1];
        for (surface, _) in &pairs {
            if let Some(slot) = entry_start.get_mut(*surface as usize + 1) {
                *slot += 1;
            }
        }
        for index in 1..entry_start.len() {
            entry_start[index] += entry_start[index - 1];
        }
        let entries = pairs
            .into_iter()
            .filter(|(surface, _)| (*surface as usize) < surfaces.len())
            .map(|(_, entry)| entry)
            .collect();
        Self {
            nodes,
            items,
            entry_start,
            entries,
            batch_rank,
            rank_batch: ranked.into_iter().map(|batch| batch as u32).collect(),
        }
    }

    fn surfaces_in_box(&self, minimum: Vec3, maximum: Vec3, mut visit: impl FnMut(u32)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let node = self.nodes[stack[top] as usize];
            if node.minimum[0] > maximum.x
                || node.maximum[0] < minimum.x
                || node.minimum[1] > maximum.y
                || node.maximum[1] < minimum.y
                || node.minimum[2] > maximum.z
                || node.maximum[2] < minimum.z
            {
                continue;
            }
            if node.count > 0 {
                let first = node.first as usize;
                for &item in &self.items[first..first + node.count as usize] {
                    visit(item);
                }
            } else {
                stack[top] = node.first;
                stack[top + 1] = node.first + 1;
                top += 2;
            }
        }
    }

    fn entries(&self, surface: u32) -> &[Entry] {
        let surface = surface as usize;
        &self.entries[self.entry_start[surface] as usize..self.entry_start[surface + 1] as usize]
    }
}

fn build_bvh(surfaces: &[scene::LegacyDlightSurface]) -> (Vec<Node>, Vec<u32>) {
    let mut items = surfaces
        .iter()
        .enumerate()
        .filter(|(_, surface)| surface.cull_kind != 0)
        .map(|(index, _)| index as u32)
        .collect::<Vec<_>>();
    let mut nodes = Vec::new();
    if items.is_empty() {
        return (nodes, items);
    }
    let centre = |item: u32, axis: usize| {
        let surface = &surfaces[item as usize];
        surface.bounds_min[axis] + surface.bounds_max[axis]
    };
    nodes.push(Node { minimum: [0.0; 3], maximum: [0.0; 3], first: 0, count: 0 });
    let mut pending = vec![(0usize, 0usize, items.len())];
    while let Some((node, start, end)) = pending.pop() {
        let mut minimum = [f32::INFINITY; 3];
        let mut maximum = [f32::NEG_INFINITY; 3];
        let mut centre_min = [f32::INFINITY; 3];
        let mut centre_max = [f32::NEG_INFINITY; 3];
        for &item in &items[start..end] {
            let surface = &surfaces[item as usize];
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(surface.bounds_min[axis]);
                maximum[axis] = maximum[axis].max(surface.bounds_max[axis]);
                let c = centre(item, axis);
                centre_min[axis] = centre_min[axis].min(c);
                centre_max[axis] = centre_max[axis].max(c);
            }
        }
        if end - start <= LEAF_SIZE {
            nodes[node] = Node { minimum, maximum, first: start as u32, count: (end - start) as u32 };
            continue;
        }
        let axis = (0..3)
            .max_by(|&a, &b| {
                (centre_max[a] - centre_min[a]).total_cmp(&(centre_max[b] - centre_min[b]))
            })
            .unwrap_or(0);
        let middle = (start + end) / 2;
        items[start..end].select_nth_unstable_by(middle - start, |&a, &b| {
            centre(a, axis).total_cmp(&centre(b, axis))
        });
        let left = nodes.len();
        nodes.push(Node { minimum: [0.0; 3], maximum: [0.0; 3], first: 0, count: 0 });
        nodes.push(Node { minimum: [0.0; 3], maximum: [0.0; 3], first: 0, count: 0 });
        nodes[node] = Node { minimum, maximum, first: left as u32, count: 0 };
        pending.push((left, start, middle));
        pending.push((left + 1, middle, end));
    }
    (nodes, items)
}

/// Light-dependent state, rewritten only when the transient lights change.
#[derive(Default)]
pub(in crate::renderer) struct LegacyDlightState {
    /// Exact mirror of the GPU surface-mask buffer.
    pub(in crate::renderer) masks: Vec<u32>,
    /// Surfaces whose mask is non-zero.
    lit: Vec<u32>,
    light_boxes: Vec<(Vec3, Vec3)>,
}

#[derive(Clone)]
pub(in crate::renderer) struct LegacyDlightDraw {
    pub(in crate::renderer) batch: u32,
    pub(in crate::renderer) indices: Range<u32>,
}

pub(in crate::renderer) struct LegacyDlightLists {
    index: OnceLock<LegacyDlightIndex>,
    pub(in crate::renderer) state: Mutex<LegacyDlightState>,
    /// This frame's static draws over `index_buffer`, in material order.
    pub(in crate::renderer) draws: Vec<LegacyDlightDraw>,
    /// This frame's lit mover batches (coarse), drawn whole from the world
    /// index buffer with the all-lights fallback.
    pub(in crate::renderer) inline_batches: Vec<u32>,
    pub(in crate::renderer) index_buffer: Option<wgpu::Buffer>,
    index_capacity: u64,
    visible: Vec<bool>,
    keys: Vec<(u32, u32)>,
    indices: Vec<u32>,
}

impl LegacyDlightLists {
    pub(in crate::renderer) fn new(surface_count: usize) -> Self {
        Self {
            index: OnceLock::new(),
            state: Mutex::new(LegacyDlightState {
                masks: vec![0; surface_count.max(1)],
                ..LegacyDlightState::default()
            }),
            draws: Vec::new(),
            inline_batches: Vec::new(),
            index_buffer: None,
            index_capacity: 0,
            visible: Vec::new(),
            keys: Vec::new(),
            indices: Vec::new(),
        }
    }

    pub(in crate::renderer) fn is_empty(&self) -> bool {
        self.draws.is_empty() && self.inline_batches.is_empty()
    }
}

fn index_for(world: &WorldGpu) -> &LegacyDlightIndex {
    world.legacy_dlight.index.get_or_init(|| {
        LegacyDlightIndex::build(
            &world.legacy_dlight_surfaces,
            &world.coarse_batches,
            &world.patch_lod,
        )
    })
}

/// Recomputes surface masks for `lights` (the first 32 transient lights, bit =
/// light index) by visiting only surfaces inside each light's box, and uploads
/// the changed span of the mask buffer.
pub(in crate::renderer) fn update_legacy_dlight_masks(
    queue: &wgpu::Queue,
    world: &WorldGpu,
    lights: &[TransientLight],
) {
    let index = index_for(world);
    let surfaces = &world.legacy_dlight_surfaces;
    let mut guard = world.legacy_dlight.state.lock().unwrap_or_else(PoisonError::into_inner);
    let state = &mut *guard;
    let mut dirty = usize::MAX..0;
    for &surface in &state.lit {
        let surface = surface as usize;
        state.masks[surface] = 0;
        dirty.start = dirty.start.min(surface);
        dirty.end = dirty.end.max(surface + 1);
    }
    state.lit.clear();
    state.light_boxes.clear();
    for (bit, light) in lights.iter().take(32).enumerate() {
        let position = Vec3::from_array(light.position);
        let radius = light.radius.max(0.0);
        let (minimum, maximum) = (position - Vec3::splat(radius), position + Vec3::splat(radius));
        state.light_boxes.push((minimum, maximum));
        let (masks, lit) = (&mut state.masks, &mut state.lit);
        index.surfaces_in_box(minimum, maximum, |surface| {
            if !legacy_dlight_touches_surface(&surfaces[surface as usize], position, radius) {
                return;
            }
            let slot = surface as usize;
            if masks[slot] == 0 {
                lit.push(surface);
                dirty.start = dirty.start.min(slot);
                dirty.end = dirty.end.max(slot + 1);
            }
            masks[slot] |= 1 << bit;
        });
    }
    if dirty.start < dirty.end {
        queue.write_buffer(
            &world.legacy_dlight_surface_mask_buffer,
            dirty.start as u64 * 4,
            bytemuck::cast_slice(&state.masks[dirty]),
        );
    }
}

/// Builds this frame's lit-triangle index list (visible lit surfaces, grouped
/// by material) and mover list. Call after the patch LOD update.
pub(in crate::renderer) fn prepare_legacy_dlight_draws(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &mut WorldGpu,
    mode: PvsMode,
    view_proj: Mat4,
) {
    let lists = &mut world.legacy_dlight;
    let mut draws = std::mem::take(&mut lists.draws);
    let mut inline_batches = std::mem::take(&mut lists.inline_batches);
    let mut visible = std::mem::take(&mut lists.visible);
    let mut keys = std::mem::take(&mut lists.keys);
    let mut indices = std::mem::take(&mut lists.indices);
    draws.clear();
    inline_batches.clear();
    keys.clear();
    indices.clear();
    collect_draws(
        world,
        mode,
        view_proj,
        &mut visible,
        &mut keys,
        &mut indices,
        &mut draws,
        &mut inline_batches,
    );
    let lists = &mut world.legacy_dlight;
    if !indices.is_empty() {
        let needed = indices.len() as u64;
        if lists.index_buffer.is_none() || lists.index_capacity < needed {
            let capacity = needed.next_power_of_two().max(MIN_INDEX_CAPACITY);
            lists.index_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA Legacy dlight lit indices"),
                size: capacity * 4,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            lists.index_capacity = capacity;
        }
        if let Some(buffer) = &lists.index_buffer {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&indices));
        }
    }
    lists.draws = draws;
    lists.inline_batches = inline_batches;
    lists.visible = visible;
    lists.keys = keys;
    lists.indices = indices;
}

#[allow(clippy::too_many_arguments)]
fn collect_draws(
    world: &WorldGpu,
    mode: PvsMode,
    view_proj: Mat4,
    visible: &mut Vec<bool>,
    keys: &mut Vec<(u32, u32)>,
    indices: &mut Vec<u32>,
    draws: &mut Vec<LegacyDlightDraw>,
    inline_batches: &mut Vec<u32>,
) {
    let state = world.legacy_dlight.state.lock().unwrap_or_else(PoisonError::into_inner);
    if state.light_boxes.is_empty() {
        return;
    }
    // One-shot receiver diagnostics at the highest renderer verbosity. This
    // distinguishes material eligibility from coarse surface rejection.
    static TRACED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if crate::logging::renderer_verbose_enabled(3)
        && !TRACED.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        for (id, batch) in world.coarse_batches.iter().enumerate() {
            if !state.light_boxes.iter().any(|(minimum, maximum)| {
                (0..3).all(|axis| batch.bounds_min[axis] <= maximum[axis]
                    && batch.bounds_max[axis] >= minimum[axis])
            }) { continue; }
            let name = world.material_debug.entries.get(batch.source.material_debug_index as usize)
                .map_or("unknown", |entry| entry.name.as_str());
            let lit = batch.legacy_dlight_runs.iter().filter(|run| {
                state.masks.get(run.surface_id as usize).is_some_and(|mask| *mask != 0)
            }).count();
            println!("[DLIGHT RECEIVER] batch={id} shader={name} eligible={} blend={:?} lightmap={} folded={} rgb={:?} lit_runs={lit}/{} depth_write={}",
                legacy_dlight_receives(&batch.source), batch.source.pipeline.blend,
                batch.source.texture_is_lightmap, batch.source.modulate_lightmap,
                batch.source.rgb_gen, batch.legacy_dlight_runs.len(), batch.source.pipeline.depth_write);
        }
    }
    let index = index_for(world);
    visible.clear();
    visible.resize(world.coarse_batches.len(), false);
    for &batch in legacy_dlight_batch_selection(world, mode).indices {
        visible[batch] = true;
    }
    for model in &world.inline_models {
        for &batch_index in &model.coarse_batches {
            let batch = &world.coarse_batches[batch_index];
            if visible[batch_index]
                && legacy_dlight_receives(&batch.source)
                && state.light_boxes.iter().any(|(minimum, maximum)| {
                    batch.bounds_min[0] <= maximum.x
                        && batch.bounds_max[0] >= minimum.x
                        && batch.bounds_min[1] <= maximum.y
                        && batch.bounds_max[1] >= minimum.y
                        && batch.bounds_min[2] <= maximum.z
                        && batch.bounds_max[2] >= minimum.z
                })
                && aabb_intersects_clip_frustum(batch.bounds_min, batch.bounds_max, view_proj)
            {
                inline_batches.push(batch_index as u32);
            }
        }
    }
    // rd-vulkan only lights surfaces that passed the view's culling.
    for &surface in &state.lit {
        let bounds = &world.legacy_dlight_surfaces[surface as usize];
        if !aabb_intersects_clip_frustum(bounds.bounds_min, bounds.bounds_max, view_proj) {
            continue;
        }
        let first = index.entry_start[surface as usize];
        for (offset, entry) in index.entries(surface).iter().enumerate() {
            if visible[entry.batch as usize] {
                keys.push((index.batch_rank[entry.batch as usize], first + offset as u32));
            }
        }
    }
    drop(state);
    keys.sort_unstable();
    // Static runs exist even on maps without curved patches. Patch LOD
    // deliberately retains no index source there; the resident world's
    // immutable source is shared by all geometry consumers.
    let source = world.remap_index_source.as_slice();
    let mut current = u32::MAX;
    for &(rank, entry) in keys.iter() {
        if rank != current {
            let start = indices.len() as u32;
            draws.push(LegacyDlightDraw {
                batch: index.rank_batch[rank as usize],
                indices: start..start,
            });
            current = rank;
        }
        match index.entries[entry as usize].piece {
            Piece::Static { first, count } => {
                if let Some(slice) = source.get(first as usize..(first + count) as usize) {
                    indices.extend_from_slice(slice);
                }
            }
            Piece::Patch { grid } => world.patch_lod.append_grid_indices(grid as usize, indices),
        }
        if let Some(draw) = draws.last_mut() {
            draw.indices.end = indices.len() as u32;
        }
    }
    draws.retain(|draw| !draw.indices.is_empty());
}
