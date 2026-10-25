//! Curved grids share the existing packed draw surfaces. Full-detail indices
//! remain immutable for secondary views, physics consumers and AUTO workers.
use crate::renderer::visibility::compaction::IndexedGeometryBuilder;
use crate::renderer::{
    aabb_intersects_clip_frustum, active_batch_selection, batch_area_visible, effective_area_mask,
    hidden_inline_batch, scene, Auto4GeometryState, Camera, DrawIndexedIndirectArgs, GpuCullRecord,
    InlineModelInstance, PvsMode, WorldBatch, WorldGpu,
};
use jka_assets::bsp::patch::PatchLod;
use std::{collections::BTreeMap, ops::Range, sync::Arc, time::Instant};

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::renderer) struct PatchLodStats {
    pub visible_patches: u32,
    pub full_triangles: u64,
    pub selected_triangles: u64,
    pub lod_updates: u32,
    pub index_upload_bytes: u64,
    pub cpu_lod_ms: f64,
}

struct Grid {
    lod: Arc<PatchLod>,
    vertices: Vec<u32>,
    model: Option<usize>,
    masks: [u128; 2],
    suppressed: bool,
    version: u64,
    source_range: Range<u32>,
    surface: usize,
    threshold: f32,
    simplifiable: bool,
    original_indices: Option<Vec<u32>>,
}

#[derive(Clone)]
enum Part {
    Static(Range<u32>),
    Patch(usize),
}

struct Geometry {
    original: Range<u32>,
    reserved: Range<u32>,
    parts: Vec<Part>,
    grids: Vec<usize>,
    versions: Vec<u64>,
    active: Range<u32>,
    /// Grid versions currently written into `reserved`, which keeps every part
    /// at its FULL offset (patches padded with degenerate triangles), so a LOD
    /// change rewrites only that patch. `None` until `reserved` is filled.
    reserved_versions: Option<Vec<u64>>,
    /// Legacy dlight surface runs in FULL-detail index space (`original`).
    original_runs: Vec<crate::renderer::LegacyDlightRun>,
    targets: Vec<(u8, usize)>,
}

#[derive(Default)]
pub(in crate::renderer) struct WorldPatchLod {
    grids: Vec<Grid>,
    geometries: Vec<Geometry>,
    ranges: BTreeMap<(u32, u32), usize>,
    /// References immutable original FULL ranges; no worker ever reads an LOD stream.
    source: Option<Arc<Vec<u32>>>,
    recipe_members: Vec<Vec<usize>>,
    recipe_versions: Vec<Vec<u64>>,
    recipe_reserved: Vec<Option<Range<u32>>>,
    dynamic_recipes: Vec<usize>,
    completed_recipes: u64,
    visible: Vec<bool>,
    needed: Vec<bool>,
    scratch: Vec<u32>,
    view_key: Option<(
        [u32; 7],
        [u32; 16],
        PvsMode,
        Option<usize>,
        Option<[u8; 32]>,
        bool,
        bool,
    )>,
    /// Inline models that own a patch grid. Only their poses affect LOD; other
    /// movers (e.g. AMJH3TE's fish) re-pose every frame and must not force a
    /// full recompute.
    patch_models: Vec<usize>,
    poses: Vec<Option<Option<InlineModelInstance>>>,
    /// Grids marked visible by the last changed pass (scratch).
    visible_ids: Vec<usize>,
    /// `suppressed` of every grid (r_nocurves of the last pass).
    grids_suppressed: bool,
    stats: PatchLodStats,
    last_log: Option<Instant>,
    pending_upload_bytes: u64,
    all_suppressed: bool,
}

impl WorldPatchLod {
    pub fn prepare(
        builder: &mut IndexedGeometryBuilder<'_>,
        world_draws: &[scene::PreparedPatchDraw],
        inline_draws: &[scene::PreparedPatchDraw],
        world_batches: impl Iterator<Item = (Range<u32>, Range<u32>)>,
        inline_batches: impl Iterator<Item = (Range<u32>, Range<u32>)>,
        inline_models: &[scene::InlineModelGeometry],
        gpu_starts: &[u32],
        triangle_surfaces: &[u32],
    ) -> Self {
        if world_draws.is_empty() && inline_draws.is_empty() { return Self::default(); }
        let mut result = Self::default();
        let mut draws = Vec::new();
        for (inline, prepared) in [(false, world_draws), (true, inline_draws)] {
            for draw in prepared {
                let model = inline.then(|| {
                    inline_models
                        .iter()
                        .position(|m| {
                            m.vertices.start <= draw.vertices.start
                                && draw.vertices.end <= m.vertices.end
                        })
                        .expect("inline patch belongs to a model")
                });
                let mut vertices = vec![u32::MAX; draw.lod.width * draw.lod.height];
                let w = draw.lod.width;
                let mut simplifiable = true;
                for y in 0..draw.lod.height - 1 {
                    for x in 0..w - 1 {
                        let offset = draw.vertices.start as usize + (y * (w - 1) + x) * 6;
                        let a = y * w + x;
                        for (n, coordinate) in [a, a + 1, a + w, a + 1, a + w + 1, a + w]
                            .into_iter()
                            .enumerate()
                        {
                            let gpu = if let Some(slot) = model {
                                gpu_starts[slot] + offset as u32 + n as u32
                                    - inline_models[slot].vertices.start
                            } else {
                                builder.source_to_index[offset + n].expect("patch vertex interned")
                            };
                            // Inline meshes are triangle lists and do not intern:
                            // any occurrence carries exactly the same attributes.
                            if vertices[coordinate] != u32::MAX
                                && bytemuck::bytes_of(
                                    &builder.vertices[vertices[coordinate] as usize],
                                ) != bytemuck::bytes_of(&builder.vertices[gpu as usize])
                            {
                                simplifiable = false;
                            }
                            vertices[coordinate] = gpu;
                        }
                    }
                }
                let original_indices = (!simplifiable).then(|| {
                    draw.vertices
                        .clone()
                        .map(|offset| {
                            if let Some(slot) = model {
                                gpu_starts[slot] + offset - inline_models[slot].vertices.start
                            } else {
                                builder.source_to_index[offset as usize].unwrap()
                            }
                        })
                        .collect()
                });
                let masks = draw.lod.selection(f32::INFINITY);
                let id = result.grids.len();
                result.grids.push(Grid {
                    lod: Arc::clone(&draw.lod),
                    vertices,
                    model,
                    masks,
                    suppressed: false,
                    version: 0,
                    source_range: draw.vertices.clone(),
                    surface: draw.surface,
                    threshold: f32::INFINITY,
                    simplifiable,
                    original_indices,
                });
                draws.push((inline, draw.vertices.clone(), id));
            }
        }
        // Per class, draws sorted by first vertex: a batch's patches are the
        // draws whose vertex range lies inside the batch's source range.
        let mut by_start = [Vec::new(), Vec::new()];
        for (index, (inline, range, _)) in draws.iter().enumerate() {
            by_start[usize::from(*inline)].push((range.start, index));
        }
        for list in &mut by_start {
            list.sort_unstable();
        }
        for (inline, batches) in [
            (false, world_batches.collect::<Vec<_>>()),
            (true, inline_batches.collect::<Vec<_>>()),
        ] {
            for (source_range, original) in batches {
                let key = (original.start, original.end);
                if result.ranges.contains_key(&key) {
                    continue;
                }
                let sorted = &by_start[usize::from(inline)];
                let first = sorted.partition_point(|&(start, _)| start < source_range.start);
                let mut patches: Vec<_> = sorted[first..]
                    .iter()
                    .take_while(|&&(start, _)| start < source_range.end)
                    .map(|&(_, index)| &draws[index])
                    .filter(|(_, range, _)| range.end <= source_range.end)
                    .collect();
                if patches.is_empty() {
                    continue;
                }
                patches.sort_unstable_by_key(|(_, range, _)| range.start);
                let mut parts = Vec::new();
                let mut grids = Vec::new();
                let mut cursor = original.start;
                for (_, range, id) in patches {
                    let start = original.start + range.start - source_range.start;
                    if cursor < start {
                        parts.push(Part::Static(cursor..start));
                    }
                    parts.push(Part::Patch(*id));
                    grids.push(*id);
                    cursor = original.start + range.end - source_range.start;
                }
                if cursor < original.end {
                    parts.push(Part::Static(cursor..original.end));
                }
                // One reusable capacity per packed geometry, shared by every
                // material stage. Never recreate vertices or pipelines for LOD.
                let start = builder.indices.len() as u32;
                let end = start
                    .checked_add(original.end - original.start)
                    .expect("patch index capacity overflow");
                builder.indices.resize(end as usize, 0);
                let reserved = start..end;
                let original_runs = if inline {
                    Vec::new()
                } else {
                    let mut runs: Vec<crate::renderer::LegacyDlightRun> = Vec::new();
                    for local in (0..source_range.end - source_range.start).step_by(3) {
                        let surface_id = triangle_surfaces
                            .get(((source_range.start + local) / 3) as usize)
                            .copied()
                            .unwrap_or(u32::MAX);
                        let index = original.start + local;
                        if let Some(last) = runs.last_mut().filter(|r| r.surface_id == surface_id) {
                            last.indexed_range.end = index + 3;
                        } else {
                            runs.push(crate::renderer::LegacyDlightRun {
                                surface_id,
                                indexed_range: index..index + 3,
                            });
                        }
                    }
                    runs
                };
                result.ranges.insert(key, result.geometries.len());
                result.geometries.push(Geometry {
                    original: original.clone(),
                    reserved,
                    parts,
                    versions: vec![0; grids.len()],
                    grids,
                    active: original,
                    reserved_versions: None,
                    original_runs,
                    targets: Vec::new(),
                });
            }
        }
        result.visible.resize(result.grids.len(), false);
        result.needed.resize(result.geometries.len(), false);
        result.patch_models = result.grids.iter().filter_map(|g| g.model).collect();
        result.patch_models.sort_unstable();
        result.patch_models.dedup();
        result
    }

    pub fn has_patches(&self) -> bool {
        !self.grids.is_empty()
    }

    pub fn finish(&mut self, source: Arc<Vec<u32>>) {
        if self.has_patches() { self.source = Some(source); }
    }

    /// Rebinding after a shader-stage count change must clear the old batch
    /// indices. The indexed geometry itself remains resident and unchanged.
    pub fn clear_batch_bindings(&mut self) {
        for geometry in &mut self.geometries {
            geometry.targets.clear();
        }
        self.dynamic_recipes.clear();
    }

    pub fn bind(&mut self, batches: &mut [WorldBatch], set: u8) {
        for (index, batch) in batches.iter_mut().enumerate() {
            batch.full_indexed_range = batch.indexed_range.clone();
            batch.patch_geometry = self
                .ranges
                .get(&(batch.indexed_range.start, batch.indexed_range.end))
                .copied();
            if let Some(g) = batch.patch_geometry {
                self.geometries[g].targets.push((set, index));
            }
        }
    }

    /// Recipes containing dynamic patches use the same lazy collapse cache as
    /// other AUTO geometry, even when their original FULL pieces were contiguous.
    pub fn recipes(
        &mut self,
        members: &[Vec<usize>],
        full: &[WorldBatch],
        states: &mut [Auto4GeometryState],
    ) {
        if !self.has_patches() { return; }
        for (i, members) in members.iter().enumerate() {
            if members
                .iter()
                .any(|&m| full.get(m).is_some_and(|b| b.patch_geometry.is_some()))
            {
                states[i] = Auto4GeometryState::Uncached;
                self.dynamic_recipes.push(i);
            }
        }
        self.recipe_members = members
            .iter()
            .map(|members| {
                members
                    .iter()
                    .filter_map(|&m| full.get(m).and_then(|b| b.patch_geometry))
                    .collect()
            })
            .collect();
        self.recipe_versions.resize_with(members.len(), Vec::new);
        self.recipe_reserved.resize(members.len(), None);
    }

    pub fn describe(&self, source_vertex: u32) -> Option<Vec<String>> {
        let grid = self
            .grids
            .iter()
            .find(|g| g.model.is_none() && g.source_range.contains(&source_vertex))?;
        let lod = &grid.lod;
        let active = if grid.suppressed {
            0
        } else {
            (grid.masks[0].count_ones() - 1) * (grid.masks[1].count_ones() - 1) * 2
        };
        Some(vec![
            format!("PATCH SURFACE: #{}", grid.surface),
            format!("Grid: {} x {}", lod.width, lod.height),
            format!(
                "LOD group: origin {:?}, radius {:.3}",
                lod.lod_origin, lod.lod_radius
            ),
            format!(
                "Full triangles: {}    Active triangles: {active}",
                lod.full_triangles()
            ),
            format!("LOD threshold: {:.6}", grid.threshold),
            format!(
                "Subdivision: {}    Stitched vertices: {}",
                lod.subdivision_error, lod.stitched_vertices
            ),
            format!(
                "Grid attributes support simplification: {}",
                grid.simplifiable
            ),
        ])
    }

    fn upload_geometry(
        &mut self,
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        geometry: usize,
    ) -> Option<(Range<u32>, u64)> {
        if !self.needs_rebuild(geometry) {
            return None;
        }
        let g = &self.geometries[geometry];
        let full = g.grids.iter().all(|&id| {
            !self.grids[id].suppressed
                && self.grids[id].masks == self.grids[id].lod.selection(f32::INFINITY)
        });
        let mut bytes = 0;
        let range = if full {
            g.original.clone()
        } else {
            // Every part keeps its FULL offset inside `reserved`, so only the
            // patches whose LOD changed since `reserved` was written are
            // uploaded: a few KB, not the whole batch (9.5 MB per mouse
            // movement on costadelsol's terrain).
            let mut indices = std::mem::take(&mut self.scratch);
            let mut offset = g.reserved.start;
            let mut slot = 0;
            for part in &g.parts {
                match part {
                    Part::Static(range) => {
                        if g.reserved_versions.is_none() {
                            queue.write_buffer(
                                buffer,
                                u64::from(offset) * 4,
                                bytemuck::cast_slice(
                                    &self.source.as_ref().expect("patch indices installed")
                                        [range.start as usize..range.end as usize],
                                ),
                            );
                            bytes += u64::from(range.end - range.start) * 4;
                        }
                        offset += range.end - range.start;
                    }
                    Part::Patch(id) => {
                        let grid = &self.grids[*id];
                        let capacity = grid.source_range.end - grid.source_range.start;
                        if g.reserved_versions.as_ref().is_none_or(|v| v[slot] != grid.version) {
                            indices.clear();
                            self.padded_patch_indices(*id, &mut indices);
                            queue.write_buffer(buffer, u64::from(offset) * 4, bytemuck::cast_slice(&indices));
                            bytes += indices.len() as u64 * 4;
                        }
                        offset += capacity;
                        slot += 1;
                    }
                }
            }
            self.scratch = indices;
            debug_assert_eq!(offset - g.reserved.start, g.original.end - g.original.start);
            g.reserved.start..offset
        };
        let g = &mut self.geometries[geometry];
        g.active = range.clone();
        for (version, &id) in g.versions.iter_mut().zip(&g.grids) {
            *version = self.grids[id].version;
        }
        if !full {
            g.reserved_versions = Some(g.versions.clone());
        }
        Some((range, bytes))
    }

    /// One patch's selected indices padded with degenerate triangles to its
    /// FULL index count (zero-area triangles are discarded before raster).
    fn padded_patch_indices(&self, id: usize, out: &mut Vec<u32>) {
        let grid = &self.grids[id];
        let capacity = (grid.source_range.end - grid.source_range.start) as usize;
        let start = out.len();
        if !grid.suppressed {
            if let Some(original) = &grid.original_indices {
                out.extend_from_slice(original);
            } else {
                grid.lod.append_indices(grid.masks, &grid.vertices, out);
            }
        }
        debug_assert!(out.len() - start <= capacity);
        let pad = grid.vertices.iter().copied().find(|&v| v != u32::MAX).unwrap_or(0);
        out.resize(start + capacity, pad);
    }

    fn needs_rebuild(&self, geometry: usize) -> bool {
        let g = &self.geometries[geometry];
        g.versions
            .iter()
            .zip(&g.grids)
            .any(|(&version, &id)| version != self.grids[id].version)
    }

    fn geometry_indices(&self, geometry: usize, out: &mut Vec<u32>) {
        for part in &self.geometries[geometry].parts {
            match part {
                Part::Static(range) => out.extend_from_slice(
                    &self.source.as_ref().expect("patch indices installed")
                        [range.start as usize..range.end as usize],
                ),
                Part::Patch(id) => {
                    let grid = &self.grids[*id];
                    if !grid.suppressed {
                        if let Some(original) = &grid.original_indices {
                            out.extend_from_slice(original);
                        } else {
                            grid.lod.append_indices(grid.masks, &grid.vertices, out);
                        }
                    }
                }
            }
        }
    }

    /// `geometry_indices` with every patch padded to its FULL slot, so each
    /// part sits at its FULL offset (see `padded_patch_indices`).
    fn padded_geometry_indices(&self, geometry: usize, out: &mut Vec<u32>) {
        for part in &self.geometries[geometry].parts {
            match part {
                Part::Static(range) => out.extend_from_slice(
                    &self.source.as_ref().expect("patch indices installed")
                        [range.start as usize..range.end as usize],
                ),
                Part::Patch(id) => self.padded_patch_indices(*id, out),
            }
        }
    }

    /// Rewrite only the patch spans of a resident recipe whose grid version
    /// differs from `written` (its last `recipe_signature`). Recipes use the
    /// padded layout, so every span sits at a fixed offset. Returns bytes.
    fn rewrite_recipe_patches(
        &mut self,
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        members: &[usize],
        full: &[WorldBatch],
        start: u32,
        written: &[u64],
    ) -> u64 {
        let mut indices = std::mem::take(&mut self.scratch);
        let (mut offset, mut slot, mut bytes) = (start, 0, 0);
        for &member in members {
            let batch = &full[member];
            let Some(geometry) = batch.patch_geometry else {
                offset += batch.full_indexed_range.end - batch.full_indexed_range.start;
                continue;
            };
            for part in &self.geometries[geometry].parts {
                match part {
                    Part::Static(range) => offset += range.end - range.start,
                    Part::Patch(id) => {
                        let grid = &self.grids[*id];
                        if written.get(slot) != Some(&grid.version) {
                            indices.clear();
                            self.padded_patch_indices(*id, &mut indices);
                            queue.write_buffer(buffer, u64::from(offset) * 4, bytemuck::cast_slice(&indices));
                            bytes += indices.len() as u64 * 4;
                        }
                        offset += grid.source_range.end - grid.source_range.start;
                        slot += 1;
                    }
                }
            }
        }
        self.scratch = indices;
        bytes
    }

    fn recipe_indices(&self, members: &[usize], full: &[WorldBatch], out: &mut Vec<u32>) {
        for &member in members {
            let batch = &full[member];
            if let Some(geometry) = batch.patch_geometry {
                self.padded_geometry_indices(geometry, out);
            } else {
                let range = &batch.full_indexed_range;
                out.extend_from_slice(
                    &self.source.as_ref().expect("patch indices installed")
                        [range.start as usize..range.end as usize],
                );
            }
        }
    }

    /// Applied to a freshly completed worker result before it reaches the GPU.
    pub fn collapse(
        &mut self,
        recipe: usize,
        members: &[usize],
        full: &[WorldBatch],
        indices: &mut Vec<u32>,
    ) -> bool {
        if self.recipe_members.get(recipe).is_none_or(Vec::is_empty) {
            return false;
        }
        indices.clear();
        self.recipe_indices(members, full, indices);
        true
    }

    pub fn note_collapse(&mut self, recipe: usize, reserved: Range<u32>, uploaded_bytes: u64) {
        if self.recipe_members.get(recipe).is_some_and(|members|!members.is_empty()) {
            self.recipe_versions[recipe] = self.recipe_signature(recipe);
            self.recipe_reserved[recipe] = Some(reserved);
            self.pending_upload_bytes += uploaded_bytes;
        }
    }

    fn recipe_signature(&self, recipe: usize) -> Vec<u64> {
        self.recipe_members[recipe]
            .iter()
            .flat_map(|&g| {
                self.geometries[g]
                    .grids
                    .iter()
                    .map(|&id| self.grids[id].version)
            })
            .collect()
    }

    /// Append patch-selected indices for one FULL batch. A false return
    /// delegates non-patch source pieces to BSP model LOD / original indices.
    pub fn append_batch_indices(&self, batch: &WorldBatch, out: &mut Vec<u32>) -> bool {
        let Some(g) = batch.patch_geometry else { return false; };
        self.geometry_indices(g, out);
        true
    }

    pub fn stats(&self) -> PatchLodStats { self.stats }

    /// Legacy dlight surface runs of a packed geometry, in FULL-detail index space.
    pub fn original_runs(&self, geometry: usize) -> &[crate::renderer::LegacyDlightRun] {
        &self.geometries[geometry].original_runs
    }

    /// `(first FULL index, FULL index count, grid)` for every patch of a geometry,
    /// in index order.
    pub fn patch_spans(&self, geometry: usize) -> Vec<(u32, u32, usize)> {
        let g = &self.geometries[geometry];
        let mut spans = Vec::with_capacity(g.grids.len());
        let mut cursor = g.original.start;
        for part in &g.parts {
            match part {
                Part::Static(r) => cursor = r.end,
                Part::Patch(id) => {
                    let full = self.grids[*id].lod.full_triangles() as u32 * 3;
                    spans.push((cursor, full, *id));
                    cursor += full;
                }
            }
        }
        spans
    }

    /// Appends the indices a grid currently draws (its selected LOD), so a
    /// depth-EQUAL redraw matches the main pass exactly.
    pub fn append_grid_indices(&self, grid: usize, out: &mut Vec<u32>) {
        let grid = &self.grids[grid];
        if grid.suppressed {
            return;
        }
        if let Some(original) = &grid.original_indices {
            out.extend_from_slice(original);
        } else {
            grid.lod.append_indices(grid.masks, &grid.vertices, out);
        }
    }
}

pub(in crate::renderer) fn update(
    queue: &wgpu::Queue,
    world: &mut WorldGpu,
    camera: &Camera,
    mode: PvsMode,
    area_mask: Option<&[u8; 32]>,
    view_proj: crate::renderer::Mat4,
    curve_error: f32,
    no_curves: bool,
    speeds: i32,
    legacy_dlights: bool,
) -> PatchLodStats {
    if world.patch_lod.grids.is_empty() {
        return PatchLodStats::default();
    }
    let diagnostics = speeds == 8 || crate::renderer::diagnostics::perf_capture::enabled();
    let started = diagnostics.then(Instant::now);
    if no_curves
        && !diagnostics
        && world.patch_lod.all_suppressed
        && world.patch_lod.completed_recipes == world.auto4_collapse_cache.completed
    {
        return PatchLodStats {
            lod_updates: 0,
            index_upload_bytes: 0,
            cpu_lod_ms: 0.0,
            ..world.patch_lod.stats
        };
    }
    let mut lod = std::mem::take(&mut world.patch_lod);
    let view = camera.position.to_array();
    let forward = camera.forward().to_array();
    let key = (
        [
            view[0],
            view[1],
            view[2],
            forward[0],
            forward[1],
            forward[2],
            curve_error,
        ]
        .map(f32::to_bits),
        view_proj.to_cols_array().map(f32::to_bits),
        mode,
        world.last_cluster.flatten(),
        area_mask.copied(),
        no_curves,
        legacy_dlights,
    );
    let poses_changed = lod.poses.len() != lod.patch_models.len()
        || lod
            .poses
            .iter()
            .zip(&lod.patch_models)
            .any(|(p, &m)| *p != world.inline_models[m].uploaded);
    let changed = lod.view_key != Some(key) || poses_changed;
    let mut stats = PatchLodStats {
        lod_updates: 0,
        index_upload_bytes: std::mem::take(&mut lod.pending_upload_bytes),
        cpu_lod_ms: 0.0,
        ..lod.stats
    };
    if changed {
        lod.view_key = Some(key);
        lod.poses.clear();
        lod.poses
            .extend(lod.patch_models.iter().map(|&m| world.inline_models[m].uploaded));
        lod.visible.fill(false);
        lod.needed.fill(false);
        let active = active_batch_selection(world, mode);
        let area = effective_area_mask(world, mode, area_mask);
        let auto4_active = std::ptr::eq(active.batches.as_ptr(), world.auto4_batches.as_ptr());
        for &index in active.indices {
            let batch = &active.batches[index];
            // Only batches that reference patch geometry matter; test that before
            // the frustum so the common patch-free batch costs two loads.
            // A resident recipe references the same exact FULL members.
            let recipe = (auto4_active && index >= world.auto4_variant_base)
                .then(|| world.auto4_variant_geometry.get(index - world.auto4_variant_base))
                .flatten()
                .map(|&recipe| lod.recipe_members[recipe].as_slice())
                .filter(|members| !members.is_empty());
            if batch.patch_geometry.is_none() && recipe.is_none() {
                continue;
            }
            if !batch_area_visible(batch, area)
                || hidden_inline_batch(batch, &world.inline_models)
                || !aabb_intersects_clip_frustum(batch.bounds_min, batch.bounds_max, view_proj)
            {
                continue;
            }
            if let Some(g) = batch.patch_geometry {
                lod.needed[g] = true;
            }
            for &g in recipe.unwrap_or_default() {
                lod.needed[g] = true;
            }
        }
        if legacy_dlights {
            // Static lit surfaces redraw from each grid's own selection, which the
            // main pass keeps current. Movers redraw their whole coarse batch range,
            // so keep visible curved movers' coarse geometry current too.
            for model in &world.inline_models {
                for &index in &model.coarse_batches {
                    let batch = &world.coarse_batches[index];
                    let Some(g) = batch.patch_geometry else { continue };
                    if batch_area_visible(batch, area)
                        && !hidden_inline_batch(batch, &world.inline_models)
                        && aabb_intersects_clip_frustum(batch.bounds_min, batch.bounds_max, view_proj)
                    {
                        lod.needed[g] = true;
                    }
                }
            }
        }
        // A diagnostic toggle affects every view, including secondary cameras.
        // Every grid's `suppressed` equals the last pass's `no_curves`, so only a
        // toggle has to touch the (many) grids outside the view.
        let suppression_changed = lod.grids_suppressed != no_curves;
        let mut visible_ids = std::mem::take(&mut lod.visible_ids);
        visible_ids.clear();
        for (g, needed) in lod.needed.iter().enumerate() {
            if *needed {
                for &id in &lod.geometries[g].grids {
                    if !lod.visible[id] {
                        lod.visible[id] = true;
                        visible_ids.push(id);
                    }
                }
            }
        }
        stats.visible_patches = 0;
        stats.full_triangles = 0;
        stats.selected_triangles = 0;
        if suppression_changed {
            for (id, grid) in lod.grids.iter_mut().enumerate() {
                if !lod.visible[id] && grid.suppressed != no_curves {
                    grid.suppressed = no_curves;
                    grid.version = grid.version.wrapping_add(1);
                    stats.lod_updates += 1;
                }
            }
            lod.grids_suppressed = no_curves;
        }
        for &id in &visible_ids {
            let grid = &mut lod.grids[id];
            stats.visible_patches += 1;
            stats.full_triangles += grid.lod.full_triangles() as u64;
            if no_curves {
                if !grid.suppressed {
                    grid.suppressed = true;
                    grid.version = grid.version.wrapping_add(1);
                    stats.lod_updates += 1;
                }
                continue;
            }
            let origin = scene::render_position(grid.lod.lod_origin);
            let origin = grid
                .model
                .and_then(|m| world.inline_models[m].uploaded.flatten())
                .map_or(origin, |pose| pose.transform_point(origin));
            // Straight-line distance, not JKA's depth along the view axis
            // (LodErrorForVolume): turning in place then never changes a
            // curve's detail. Screen-edge curves get slightly less detail
            // than JKA would give them.
            let depth = (0..3)
                .map(|i| (origin[i] - view[i]).powi(2))
                .sum::<f32>()
                .sqrt();
            let threshold = if curve_error < 0.0 {
                0.0
            } else {
                curve_error / (depth - grid.lod.lod_radius).max(1.0)
            };
            // Hysteresis: keep the current detail until the allowed error has
            // moved more than LOD_HYSTERESIS from the one it was picked with.
            // Small mouse circles otherwise flip curves near a level boundary
            // back and forth, re-uploading them every frame. (JKA re-picks
            // exactly each frame; a 5% error band is not visible.)
            const LOD_HYSTERESIS: f32 = 0.05;
            if no_curves == grid.suppressed
                && grid.threshold.is_finite()
                && (threshold - grid.threshold).abs() <= grid.threshold * LOD_HYSTERESIS
            {
                if !grid.suppressed {
                    stats.selected_triangles += (grid.masks[0].count_ones() as u64 - 1)
                        * (grid.masks[1].count_ones() as u64 - 1) * 2;
                }
                continue;
            }
            grid.threshold = threshold;
            let masks = grid.lod.selection(if grid.simplifiable {
                threshold
            } else {
                f32::INFINITY
            });
            if masks != grid.masks || no_curves != grid.suppressed {
                grid.masks = masks;
                grid.suppressed = no_curves;
                grid.version = grid.version.wrapping_add(1);
                stats.lod_updates += 1;
            }
            if !grid.suppressed {
                stats.selected_triangles +=
                    (masks[0].count_ones() as u64 - 1) * (masks[1].count_ones() as u64 - 1) * 2;
            }
        }
        lod.visible_ids = visible_ids;
        for geometry in 0..lod.geometries.len() {
            if !lod.needed[geometry] && !suppression_changed {
                continue;
            }
            let Some((range, bytes)) = lod.upload_geometry(queue, &world.index_buffer, geometry)
            else {
                continue;
            };
            stats.index_upload_bytes += bytes;
            let g = &lod.geometries[geometry];
            for &(set, index) in &g.targets {
                let batches = match set {
                    0 => &mut world.coarse_batches,
                    1 => &mut world.full_batches,
                    _ => &mut world.auto4_batches,
                };
                let batch = &mut batches[index];
                // The padded layout keeps a partial LOD's range fixed; only a
                // switch to or from FULL detail moves it.
                if batch.indexed_range == range && batch.patch_suppressed == no_curves {
                    continue;
                }
                batch.indexed_range = range.clone();
                batch.patch_suppressed = no_curves;
                upload_batch_range(
                    queue,
                    &world.cull_records_buffer,
                    &world.indirect_buffer,
                    batch,
                );
            }
        }
    }
    // A new lazy collapse may arrive on an otherwise unchanged camera frame.
    // Repack only resident recipes whose members changed or whose cache slot moved.
    let recipe_changes =
        stats.lod_updates != 0 || lod.completed_recipes != world.auto4_collapse_cache.completed;
    lod.completed_recipes = world.auto4_collapse_cache.completed;
    for recipe_index in 0..if recipe_changes {
        lod.dynamic_recipes.len()
    } else {
        0
    } {
        let recipe = lod.dynamic_recipes[recipe_index];
        let Some(Auto4GeometryState::Ready { range, .. }) =
            world.auto4_collapse_cache.states.get(recipe)
        else {
            continue;
        };
        let range = range.clone();
        let signature = lod.recipe_signature(recipe);
        if lod.recipe_reserved[recipe].as_ref() == Some(&range) {
            if lod.recipe_versions[recipe] == signature {
                continue;
            }
            // Same slot, padded layout: rewrite just the patches that changed.
            // The recipe's draw range is unchanged.
            let written = std::mem::take(&mut lod.recipe_versions[recipe]);
            stats.index_upload_bytes += lod.rewrite_recipe_patches(
                queue, &world.index_buffer, &world.auto4_geometry_members[recipe],
                &world.full_batches, range.start, &written,
            );
            lod.recipe_versions[recipe] = signature;
            continue;
        }
        let mut indices = std::mem::take(&mut lod.scratch);
        indices.clear();
        lod.recipe_indices(
            &world.auto4_geometry_members[recipe],
            &world.full_batches,
            &mut indices,
        );
        assert!(
            indices.len() <= (range.end - range.start) as usize,
            "LOD recipe exceeds reserved full detail"
        );
        if !indices.is_empty() {
            queue.write_buffer(
                &world.index_buffer,
                u64::from(range.start) * 4,
                bytemuck::cast_slice(&indices),
            );
            stats.index_upload_bytes += indices.len() as u64 * 4;
        }
        let active = range.start..range.start + indices.len() as u32;
        crate::renderer::visibility::compaction::auto4_set_geometry_range(
            queue, world, recipe, active,
        );
        lod.recipe_reserved[recipe] = Some(range);
        lod.recipe_versions[recipe] = signature;
        lod.scratch = indices;
    }
    stats.cpu_lod_ms = started.map_or(0.0, |start| start.elapsed().as_secs_f64() * 1000.0);
    if speeds == 8 {
        if lod
            .last_log
            .is_none_or(|last| last.elapsed().as_secs_f32() >= 1.0)
        {
            let reduction = if stats.full_triangles == 0 {
                0.0
            } else {
                100.0 * (1.0 - stats.selected_triangles as f64 / stats.full_triangles as f64)
            };
            eprintln!("[JKA PATCH LOD] visible_patches={} full_triangles={} selected_triangles={} reduction={:.1}% lod_updates={} index_upload_bytes={} cpu_lod_ms={:.4}",stats.visible_patches,stats.full_triangles,stats.selected_triangles,reduction,stats.lod_updates,stats.index_upload_bytes,stats.cpu_lod_ms);
            lod.last_log = Some(Instant::now());
        }
    }
    lod.all_suppressed = no_curves;
    lod.stats = stats;
    world.patch_lod = lod;
    stats
}

fn upload_batch_range(
    queue: &wgpu::Queue,
    records: &wgpu::Buffer,
    indirect: &wgpu::Buffer,
    batch: &WorldBatch,
) {
    let range = &batch.indexed_range;
    let draw = [range.end - range.start, range.start];
    queue.write_buffer(
        records,
        u64::from(batch.cull_index) * std::mem::size_of::<GpuCullRecord>() as u64
            + std::mem::offset_of!(GpuCullRecord, draw) as u64,
        bytemuck::cast_slice(&draw),
    );
    let args = DrawIndexedIndirectArgs {
        index_count: draw[0],
        instance_count: 1,
        first_index: draw[1],
        base_vertex: 0,
        first_instance: 0,
    };
    queue.write_buffer(
        indirect,
        u64::from(batch.cull_index) * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
        bytemuck::bytes_of(&args),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::GpuVertex;

    fn fixture() -> (Vec<GpuVertex>, Vec<scene::PreparedPatchDraw>, Vec<u32>) {
        let lod = Arc::new(PatchLod {
            width: 5,
            height: 3,
            width_lod_error: vec![0.0, 0.5, 0.125, 0.5, 0.0],
            height_lod_error: vec![0.0, 0.25, 0.0],
            lod_origin: [0.0; 3],
            lod_radius: 0.0,
            subdivision_error: 4.0,
            stitched_vertices: 0,
        });
        let mut source = Vec::new();
        let mut draws = Vec::new();
        let mut surfaces = Vec::new();
        for surface in 0..2 {
            let start = source.len() as u32;
            let vertices = (0..15)
                .map(|i| GpuVertex {
                    position: [(i % 5) as f32 + surface as f32 * 10.0, 0.0, (i / 5) as f32],
                    normal: [0.0, 1.0, 0.0],
                    uv: [(i % 5) as f32 * 0.25, (i / 5) as f32 * 0.5],
                    lightmap_uv: [0.0; 2],
                    color: [1.0; 4],
                    alpha_cutoff: 1.0,
                })
                .collect::<Vec<_>>();
            let mut indices = Vec::new();
            lod.append_indices(
                lod.selection(f32::INFINITY),
                &(0..15).collect::<Vec<_>>(),
                &mut indices,
            );
            source.extend(indices.iter().map(|&i| vertices[i as usize]));
            surfaces.extend(std::iter::repeat_n(surface, indices.len() / 3));
            draws.push(scene::PreparedPatchDraw {
                surface: surface as usize,
                vertices: start..source.len() as u32,
                lod: Arc::clone(&lod),
            });
        }
        source.extend_from_within(0..3);
        surfaces.push(17);
        (source, draws, surfaces)
    }

    #[test]
    fn inline_grids_reference_the_movers_private_triangle_vertices() {
        let (source, draws, surfaces) = fixture();
        let mut builder = IndexedGeometryBuilder::new(&[], &[]);
        builder.vertices = source.clone();
        let original = builder.append_indices(&(0..source.len() as u32).collect::<Vec<_>>());
        let models = [scene::InlineModelGeometry {model:1, vertices:0..source.len() as u32, batches:0..1}];
        let mut lod = WorldPatchLod::prepare(&mut builder,&[],&draws,std::iter::empty(),std::iter::once((0..source.len() as u32,original)),&models,&[0],&surfaces);
        lod.finish(Arc::new(builder.indices));
        assert!(lod.grids.iter().all(|g|g.model==Some(0)&&g.simplifiable));
        let mut indices=Vec::new();lod.geometry_indices(0,&mut indices);
        assert_eq!(indices.len(),source.len());
        for (&index, expected) in indices.iter().zip(&source) {assert_eq!(bytemuck::bytes_of(&builder.vertices[index as usize]),bytemuck::bytes_of(expected));}
        for grid in &mut lod.grids {grid.masks=grid.lod.selection(0.0);}
        indices.clear();lod.geometry_indices(0,&mut indices);assert_eq!(indices.len(),15);
    }

    #[test]
    fn worlds_without_patches_retain_no_extra_index_source() {
        let mut builder=IndexedGeometryBuilder::new(&[],&[]);
        let mut lod=WorldPatchLod::prepare(&mut builder,&[],&[],std::iter::empty(),std::iter::empty(),&[],&[],&[]);
        let source=Arc::new(vec![0,1,2]);
        lod.finish(Arc::clone(&source));assert_eq!(Arc::strong_count(&source),1);
        assert!(!lod.has_patches());
        // Existing AUTO workers can finish on an ordinary all-face map.
        lod.note_collapse(0,0..3,12);
        assert_eq!(lod.pending_upload_bytes,0);
    }

    #[test]
    #[ignore = "requires a native WGPU adapter"]
    fn gpu_patch_indices_upload_once_and_preserve_original_ranges() {
        crate::renderer::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            println!("PATCH GPU adapter: {:?}", adapter.get_info());
            let (source, draws, surfaces) = fixture();
            let mut builder = IndexedGeometryBuilder::new(&source, &surfaces);
            let mut full = Vec::new();
            for (i, &vertex) in source.iter().enumerate() {
                let gpu = builder.intern(vertex, surfaces[i / 3]);
                builder.source_to_index[i] = Some(gpu);
                full.push(gpu);
            }
            let original = builder.append_indices(&full);
            let mut lod = WorldPatchLod::prepare(
                &mut builder,
                &draws,
                &[],
                std::iter::once((0..source.len() as u32, original.clone())),
                std::iter::empty(),
                &[],
                &[],
                &surfaces,
            );
            lod.finish(Arc::new(builder.indices));
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("patch GPU test"),
                size: lod.source.as_ref().unwrap().len() as u64 * 4,
                usage: wgpu::BufferUsages::INDEX
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&full));
            for grid in &mut lod.grids {
                grid.masks = grid.lod.selection(0.0);
                grid.version += 1;
            }
            let (range, bytes) = lod.upload_geometry(&queue, &buffer, 0).unwrap();
            // FULL-size layout: every part keeps its offset, patches padded.
            assert_eq!(range.end - range.start, original.end - original.start);
            assert_eq!(bytes, u64::from(range.end - range.start) * 4, "first upload fills the range");
            assert!(
                lod.upload_geometry(&queue, &buffer, 0).is_none(),
                "unchanged LOD must issue no upload"
            );
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("patch index readback"),
                size: full.len() as u64 * 4 + bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(&buffer, 0, &readback, 0, full.len() as u64 * 4);
            encoder.copy_buffer_to_buffer(
                &buffer,
                u64::from(range.start) * 4,
                &readback,
                full.len() as u64 * 4,
                bytes,
            );
            let submission = queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(15)),
                })
                .unwrap();
            rx.recv().unwrap().unwrap();
            let mapped = readback.slice(..).get_mapped_range().unwrap();
            let indices: &[u32] = bytemuck::cast_slice(&mapped);
            assert_eq!(&indices[..full.len()], full);
            let mut selected = Vec::new();
            lod.geometry_indices(0, &mut selected);
            // Dropping the zero-area padding leaves exactly the compact selection.
            let drawn: Vec<u32> = indices[full.len()..]
                .chunks_exact(3)
                .filter(|t| !(t[0] == t[1] && t[1] == t[2]))
                .flatten()
                .copied()
                .collect();
            assert_eq!(drawn, selected);
            drop(mapped);
            readback.unmap();
            // A later LOD change rewrites only patch spans, never static parts.
            let patch_capacity: u64 = lod.grids.iter()
                .map(|g| u64::from(g.source_range.end - g.source_range.start) * 4).sum();
            for grid in &mut lod.grids {
                grid.version += 1;
            }
            let (again, bytes) = lod.upload_geometry(&queue, &buffer, 0).unwrap();
            assert_eq!(again, range);
            assert!(bytes <= patch_capacity, "{bytes} > {patch_capacity}");
            for grid in &mut lod.grids {
                grid.masks = grid.lod.selection(f32::INFINITY);
                grid.version += 1;
            }
            let (range, bytes) = lod.upload_geometry(&queue, &buffer, 0).unwrap();
            assert_eq!(range, original);
            assert_eq!(bytes, 0, "restoring full detail reuses immutable indices");
        });
    }

    #[test]
    fn packed_patch_streams_keep_static_triangles_and_cache_unchanged_selections() {
        let (source, draws, surfaces) = fixture();
        let mut builder = IndexedGeometryBuilder::new(&source, &surfaces);
        let mut full = Vec::new();
        for (index, &vertex) in source.iter().enumerate() {
            let gpu = builder.intern(vertex, surfaces[index / 3]);
            builder.source_to_index[index] = Some(gpu);
            full.push(gpu);
        }
        let original = builder.append_indices(&full);
        let mut lod = WorldPatchLod::prepare(
            &mut builder,
            &draws,
            &[],
            std::iter::once((0..source.len() as u32, original.clone())),
            std::iter::empty(),
            &[],
            &[],
            &surfaces,
        );
        lod.finish(Arc::new(builder.indices));
        assert_eq!(
            lod.geometries.len(),
            1,
            "one packed draw, not one draw per patch"
        );
        assert!(!lod.needs_rebuild(0));
        let mut indices = Vec::new();
        lod.geometry_indices(0, &mut indices);
        assert_eq!(indices, full);
        for (threshold, expected_patch_indices) in [(0.0, 6), (0.125, 12), (0.25, 24), (0.5, 48)] {
            for grid in &mut lod.grids {
                let masks = grid.lod.selection(threshold);
                if masks != grid.masks {
                    grid.masks = masks;
                    grid.version += 1;
                }
            }
            assert!(lod.needs_rebuild(0));
            indices.clear();
            lod.geometry_indices(0, &mut indices);
            assert_eq!(indices.len(), expected_patch_indices * 2 + 3);
            assert_eq!(&indices[indices.len() - 3..], &full[full.len() - 3..]);
            assert!(
                indices.len()
                    <= (lod.geometries[0].reserved.end - lod.geometries[0].reserved.start) as usize
            );
            let ids = lod.geometries[0].grids.clone();
            for (version, &id) in lod.geometries[0].versions.iter_mut().zip(&ids) {
                *version = lod.grids[id].version;
            }
            assert!(
                !lod.needs_rebuild(0),
                "unchanged selection must skip the upload path"
            );
            let versions = lod.grids.iter().map(|g| g.version).collect::<Vec<_>>();
            for grid in &lod.grids {
                assert_eq!(grid.masks, grid.lod.selection(threshold));
            }
            assert_eq!(
                versions,
                lod.grids.iter().map(|g| g.version).collect::<Vec<_>>()
            );
        }
        for grid in &mut lod.grids {
            grid.suppressed = true;
            grid.version += 1;
        }
        indices.clear();
        lod.geometry_indices(0, &mut indices);
        assert_eq!(
            indices,
            &full[full.len() - 3..],
            "r_nocurves preserves nonpatch geometry"
        );
        assert_eq!(
            &lod.source.as_ref().unwrap()[original.start as usize..original.end as usize],
            full,
            "full-detail index data stays immutable"
        );
    }

    #[test]
    fn triangle_local_attributes_conservatively_keep_full_patch_geometry() {
        let (mut source, draws, surfaces) = fixture();
        // A deformation can encode different data on occurrences of one point.
        source[3].normal = [1.0, 0.0, 0.0];
        let mut builder = IndexedGeometryBuilder::new(&source, &surfaces);
        let mut full = Vec::new();
        for (index, &v) in source.iter().enumerate() {
            let gpu = builder.intern(v, surfaces[index / 3]);
            builder.source_to_index[index] = Some(gpu);
            full.push(gpu);
        }
        let original = builder.append_indices(&full);
        let mut lod = WorldPatchLod::prepare(
            &mut builder,
            &draws,
            &[],
            std::iter::once((0..source.len() as u32, original)),
            std::iter::empty(),
            &[],
            &[],
            &surfaces,
        );
        lod.finish(Arc::new(builder.indices));
        assert!(!lod.grids[0].simplifiable);
        assert!(lod.grids[1].simplifiable);
        let mut indices = Vec::new();
        lod.geometry_indices(0, &mut indices);
        assert_eq!(indices, full);
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with retail PK3s"]
    fn real_bsp_preparation_preserves_packing_and_physics() {
        let base =
            std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("JKA_TEST_BASE"));
        let name = std::env::var("JKA_TEST_MAP").unwrap_or_else(|_| "mp/duel3".into());
        let mut physics = None;
        for subdivisions in [1.0_f32, 4.0, 12.0] {
            for stitching in [false, true] {
                let options = scene::MapPrepareOptions {
                    patch_subdivisions_bits: subdivisions.to_bits(),
                    patch_stitching: stitching,
                    grass: false,
                    voxel_probe_gi: false,
                    ocean: false,
                    client_physics: true,
                    ..Default::default()
                };
                let map = scene::prepare_with_options(&base, None, &name, options).unwrap();
                assert!(!map.patch_draws.is_empty(), "choose a map with patches");
                let actual = map.physics_collision.triangle_data();
                assert!(actual.is_some(), "test map must contain physical triangles");
                if let Some(expected) = &physics {
                    assert_eq!(&actual, expected, "visual cvars changed Rapier geometry");
                } else {
                    physics = Some(actual);
                }
                let mut builder = IndexedGeometryBuilder::new(
                    &map.vertices,
                    &map.legacy_dlight_triangle_surfaces,
                );
                let ranges = map
                    .batches
                    .iter()
                    .chain(&map.pvs_batches)
                    .map(|b| (b.vertices.clone(), builder.batch_range(b)))
                    .collect::<Vec<_>>();
                let mut lod = WorldPatchLod::prepare(
                    &mut builder,
                    &map.patch_draws,
                    &[],
                    ranges.into_iter(),
                    std::iter::empty(),
                    &[],
                    &[],
                    &map.legacy_dlight_triangle_surfaces,
                );
                lod.finish(Arc::new(builder.indices));
                let mut scratch = Vec::new();
                for g in 0..lod.geometries.len() {
                    scratch.clear();
                    lod.geometry_indices(g, &mut scratch);
                    let range = &lod.geometries[g].original;
                    assert_eq!(
                        scratch,
                        &lod.source.as_ref().unwrap()[range.start as usize..range.end as usize]
                    );
                }
                let full = lod
                    .grids
                    .iter()
                    .map(|g| g.lod.full_triangles())
                    .sum::<usize>();
                let mut selected = Vec::new();
                for curve_error in [25.0_f32, 250.0, 2500.0] {
                    selected.push(
                        lod.grids
                            .iter()
                            .map(|g| {
                                let m = g.lod.selection(curve_error / 4096.0);
                                (m[0].count_ones() - 1) as usize
                                    * (m[1].count_ones() - 1) as usize
                                    * 2
                            })
                            .sum::<usize>(),
                    );
                }
                assert!(selected.windows(2).all(|p| p[0] <= p[1]));
                assert!(selected[2] <= full);
                println!("PATCH REAL MAP {name} subdivisions={subdivisions} stitching={stitching} patches={} full_triangles={full} selected_at_depth4096={selected:?} coarse_batches={} full_batches={}",lod.grids.len(),map.batches.len(),map.pvs_batches.len());
            }
        }
    }
}
