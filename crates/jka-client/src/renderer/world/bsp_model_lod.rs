//! Conservative index-only distance LOD for *compiled* BSP model surfaces.
//!
//! q3map2-baked MD3s no longer have MD3 instance handles. We use verified
//! MST_TRIANGLE_SOUP + models/ surface IDs, retaining the original BSP indices
//! for AUTO-4 collapse, RT, collision, marks and legacy projected lights.
//! An independently simplified source-surface run can change LOD without
//! splitting an existing material/PVS draw into multiple draw calls.
use crate::renderer::visibility::compaction::IndexedGeometryBuilder;
use crate::renderer::{Camera, DrawBatch, DrawClass, DrawIndexedIndirectArgs, GpuCullRecord, GpuVertex, WorldBatch, WorldGpu};
use jka_assets::shader::TcGen;
use std::{collections::BTreeMap, ops::Range, sync::Arc};

const MIN_INDICES: usize = 48;
// Same triangle budgets as the MD3 fallback, but conservative about edges.
const RATIOS: [f32; 2] = [0.55, 0.25];
const ERRORS: [f32; 2] = [0.01, 0.035];
const LOCK_BORDER: u32 = 1; // meshopt_SimplifyLockBorder

struct ModelPart {
    original: Range<u32>,
    reduced: [Vec<u32>; 2],
    centre: [f32; 3],
    radius: f32,
    selected: usize,
}

enum Part {
    Untouched(Range<u32>),
    Model(ModelPart),
}

struct Geometry {
    original: Range<u32>,
    reserved: Range<u32>,
    parts: Vec<Part>,
    // 0 = coarse, 1 = full, 2 = AUTO4's FULL-piece aliases.
    targets: Vec<(u8, usize)>,
    current: Range<u32>,
    version: u64,
}

#[derive(Default)]
pub(in crate::renderer) struct BspModelLod {
    geometries: Vec<Geometry>,
    original_indices: Option<Arc<Vec<u32>>>,
    last_view: Option<([i32; 3], [i32; 3], i32, u32, i32, bool, bool)>,
    last_report: Option<([usize; 3], usize, usize)>,
    /// AUTO4 geometries containing one of our FULL source batches.
    recipe_members: Vec<Vec<usize>>,
    recipe_versions: Vec<Vec<u64>>,
    recipe_reservations: Vec<Option<Range<u32>>>,
    recipe_has_patches: Vec<bool>,
    completed_recipes: u64,
    range_lookup: BTreeMap<(u32, u32), usize>,
}

impl BspModelLod {
    pub fn source_indices(&self) -> Option<&[u32]> {
        self.original_indices.as_deref().map(Vec::as_slice)
    }

    pub fn prepare(
        builder: &mut IndexedGeometryBuilder<'_>,
        coarse: &[DrawBatch], coarse_ranges: &[Range<u32>],
        full: &[DrawBatch], full_ranges: &[Range<u32>],
        compiled_surfaces: &[bool],
        patches: &[crate::scene::PreparedPatchDraw],
        enabled: bool,
    ) -> Self {
        // Do not simplify or reserve duplicate indices when the feature is off.
        // Enabling it afterward requires the next map load, which avoids
        // a surprise synchronous simplification on the render/gameplay thread.
        if !enabled || compiled_surfaces.is_empty() { return Self::default(); }
        let mut result = Self::default();
        let mut cache = BTreeMap::<(u32, u32), usize>::new();
        let candidate_surfaces = compiled_surfaces.iter().filter(|&&is_model| is_model).count();
        let mut rejected_batches = 0usize;
        let mut candidate_pieces = 0usize;
        let mut too_small = 0usize;
        let mut failed_simplification = 0usize;
        for (set, sources, ranges) in [(0, coarse, coarse_ranges), (1, full, full_ranges)] {
            for (batch_index, (source, original)) in sources.iter().zip(ranges).enumerate() {
                // Only ordinary, opaque static geometry. In particular, shader
                // billboards encode their pivot in spare vertex attributes;
                // changing triangle topology would break that encoding.
                if source.pipeline.class != DrawClass::Opaque
                    || source.pipeline.deform_move
                    || (source.pipeline.auto_sprite || source.pipeline.auto_sprite2)
                    || !matches!(source.tc_gen, TcGen::Base)
                    || source.water || source.water_deformed
                    || source.planar_reflection || source.authored_environment_composite
                    || original.end <= original.start
                    || source.vertices.start % 3 != 0
                    || source.vertices.end % 3 != 0
                    || patches.iter().any(|p| p.vertices.start < source.vertices.end && source.vertices.start < p.vertices.end)
                { rejected_batches += 1; continue; }
                let key = (source.vertices.start, source.vertices.end);
                if let Some(&geometry) = cache.get(&key) {
                    result.geometries[geometry].targets.push((set, batch_index));
                    continue;
                }
                let mut parts = Vec::new();
                let mut has_reduction = false;
                let mut cursor = original.start;
                while cursor + 3 <= original.end {
                    let tri = (source.vertices.start + cursor - original.start) / 3;
                    let surface = builder.source_triangle_surfaces.get(tri as usize).copied().unwrap_or(u32::MAX);
                    let start = cursor;
                    cursor += 3;
                    while cursor + 3 <= original.end {
                        let next_tri = (source.vertices.start + cursor - original.start) / 3;
                        if builder.source_triangle_surfaces.get(next_tri as usize).copied() != Some(surface) { break; }
                        cursor += 3;
                    }
                    let span = start..cursor;
                    if compiled_surfaces.get(surface as usize).copied() != Some(true) {
                        parts.push(Part::Untouched(span));
                        continue;
                    }
                    candidate_pieces += 1;
                    if (span.end - span.start) < MIN_INDICES as u32 {
                        too_small += 1;
                        parts.push(Part::Untouched(span));
                        continue;
                    }
                    let original_indices = &builder.indices[span.start as usize..span.end as usize];
                    match simplify(original_indices, &builder.vertices) {
                        Some((reduced, centre, radius)) => {
                            has_reduction = true;
                            parts.push(Part::Model(ModelPart { original: span, reduced, centre, radius, selected: 0 }));
                        }
                        None => { failed_simplification += 1; parts.push(Part::Untouched(span)); },
                    }
                }
                if !has_reduction { continue; }
                // Reserve no more than the original full index count. LODs
                // concatenate into this single range, so draw count stays one.
                // Keep it after immutable static and patch reservations.
                let count = (original.end - original.start) as usize;
                let reserved = builder.append_indices(&vec![0; count]);
                let id = result.geometries.len();
                result.geometries.push(Geometry {
                    original: original.clone(), current: original.clone(),
                    reserved, parts, version: 0, targets: vec![(set, batch_index)],
                });
                cache.insert(key, id);
            }
        }
        let original_triangles: usize = result.geometries.iter().map(|g| (g.original.end - g.original.start) as usize / 3).sum();
        let reduced_parts: usize = result.geometries.iter().flat_map(|g| &g.parts)
            .filter(|p| matches!(p, Part::Model(_))).count();
        println!("BSP compiled-model LOD: {} model-path triangle surfaces, {} observed candidate pieces, {} too small, {} rejected by simplifier, {} excluded draw batches, {} reducible pieces in {} geometry groups ({} source tris)",
            candidate_surfaces, candidate_pieces, too_small, failed_simplification,
            rejected_batches, reduced_parts, result.geometries.len(), original_triangles);
        result
    }

    /// Call only after both normal WorldBatch arrays have been constructed.
    pub fn bind(&mut self, coarse: &[WorldBatch], full: &[WorldBatch]) {
        // Recalculate stage indices from the immutable resident index ranges.
        // A one-stage replacement can shift every subsequent flattened batch
        // index, so retaining the original (set, batch index) pairs is wrong.
        for geometry in &mut self.geometries {
            let original = &geometry.original;
            geometry.targets.clear();
            for (set, batches) in [(0, coarse), (1, full)] {
                geometry.targets.extend(batches.iter().enumerate().filter_map(|(index, batch)| {
                    (batch.patch_geometry.is_none() && &batch.full_indexed_range == original)
                        .then_some((set, index))
                }));
            }
        }
        self.geometries.retain(|g| !g.targets.is_empty());
        self.range_lookup = self.geometries.iter().enumerate()
            .map(|(id, g)| ((g.original.start, g.original.end), id)).collect();
    }

    /// AUTO4 recipes with compiled-model pieces cannot keep the immutable
    /// contiguous FULL alias: use the existing lazy physical-collapse worker.
    /// Material groups and their one-draw submission remain unchanged.
    pub fn recipes(
        &mut self, members: &[Vec<usize>], full: &[WorldBatch],
        states: &mut [crate::renderer::Auto4GeometryState],
    ) {
        self.recipe_members = members.iter().map(|recipe| {
            recipe.iter().filter_map(|&m| {
                let r = &full.get(m)?.full_indexed_range;
                self.range_lookup.get(&(r.start, r.end)).copied()
            }).collect::<Vec<_>>()
        }).collect();
        self.recipe_versions.resize_with(members.len(), Vec::new);
        self.recipe_reservations.resize(members.len(), None);
        self.recipe_has_patches = members.iter().map(|recipe| recipe.iter().any(|&m| {
            full.get(m).is_some_and(|b| b.patch_geometry.is_some())
        })).collect();
        for (i, list) in self.recipe_members.iter().enumerate() {
            if !list.is_empty() {
                states[i] = crate::renderer::Auto4GeometryState::Uncached;
            }
        }
    }

    pub fn auto4_extra_indices(&self) -> u64 {
        self.geometries.iter().map(|g| u64::from(g.original.end - g.original.start)).sum()
    }

    pub fn has_recipe(&self, recipe: usize) -> bool {
        self.recipe_members.get(recipe).is_some_and(|v| !v.is_empty())
    }

    /// Use the live LOD stream, never the worker's immutable original stream.
    /// Called only for geometry without patch LOD (those are owned by patch_lod).
    pub fn append_batch_indices(&self, original: &Range<u32>, out: &mut Vec<u32>) -> bool {
        let Some(&id) = self.range_lookup.get(&(original.start, original.end)) else {
            return false;
        };
        let g = &self.geometries[id];
        let Some(source) = self.original_indices.as_deref() else { return false; };
        for part in &g.parts {
            match part {
                Part::Untouched(r) => out.extend_from_slice(&source[r.start as usize..r.end as usize]),
                Part::Model(m) => {
                    if m.selected == 0 {
                        out.extend_from_slice(&source[m.original.start as usize..m.original.end as usize]);
                    } else {
                        out.extend_from_slice(&m.reduced[m.selected - 1]);
                    }
                }
            }
        }
        true
    }

    fn recipe_signature(&self, recipe: usize) -> Vec<u64> {
        self.recipe_members[recipe].iter().map(|&g| self.geometries[g].version).collect()
    }

    pub fn finish(&mut self, original_indices: Arc<Vec<u32>>) {
        if !self.geometries.is_empty() { self.original_indices = Some(original_indices); }
    }
}

/// Build a reduced index set preserving GPU vertex IDs and all authored
/// attributes. Locked borders protect silhouette, gaps and surface seams.
fn simplify(input: &[u32], vertices: &[GpuVertex]) -> Option<([Vec<u32>; 2], [f32; 3], f32)> {
    use std::collections::HashMap;
    if input.len() < MIN_INDICES || input.len() % 3 != 0 { return None; }
    let mut map = HashMap::<u32, u32>::new();
    let mut gpu_ids = Vec::new();
    let mut local = Vec::with_capacity(input.len());
    for &id in input {
        if id as usize >= vertices.len() { return None; }
        let local_id = *map.entry(id).or_insert_with(|| {
            let local_id = gpu_ids.len() as u32;
            gpu_ids.push(id);
            local_id
        });
        local.push(local_id);
    }
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    let mut positions = Vec::<[f32; 3]>::with_capacity(gpu_ids.len());
    let mut attributes = Vec::<[f32; 11]>::with_capacity(gpu_ids.len());
    for &id in &gpu_ids {
        let v = &vertices[id as usize];
        if !v.position.iter().chain(v.normal.iter()).chain(v.uv.iter())
            .chain(v.lightmap_uv.iter()).chain(v.color.iter()).all(|f| f.is_finite()) { return None; }
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(v.position[axis]);
            maximum[axis] = maximum[axis].max(v.position[axis]);
        }
        positions.push(v.position);
        attributes.push([
            v.normal[0], v.normal[1], v.normal[2],
            v.uv[0], v.uv[1],
            v.lightmap_uv[0], v.lightmap_uv[1],
            v.color[0], v.color[1], v.color[2], v.color[3],
        ]);
    }
    let size = [0, 1, 2].map(|i| maximum[i] - minimum[i]);
    // Avoid reducing a large, scattered batch as if it were a single MD3.
    // Single q3map2-baked model surfaces normally fit within this extent.
    if size.iter().copied().fold(0.0_f32, f32::max) > 4096.0 { return None; }
    let centre = [0, 1, 2].map(|i| (minimum[i] + maximum[i]) * 0.5);
    let radius = (size.iter().map(|x| x * x).sum::<f32>()).sqrt() * 0.5;
    if radius <= 0.0 { return None; }
    let weights = [0.5_f32, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0, 1.5, 1.5, 1.5, 1.5];
    let mut reduced = [Vec::new(), Vec::new()];
    for lod in 0..2 {
        let mut output = vec![0u32; local.len()];
        let target = ((local.len() as f32 * RATIOS[lod]) as usize / 3 * 3).max(3);
        let count = unsafe {
            meshopt::ffi::meshopt_simplifyWithAttributes(
                output.as_mut_ptr(), local.as_ptr(), local.len(),
                positions.as_ptr().cast::<f32>(), positions.len(), 12,
                attributes.as_ptr().cast::<f32>(), 44,
                weights.as_ptr(), weights.len(), std::ptr::null(),
                target, ERRORS[lod], LOCK_BORDER, std::ptr::null_mut(),
            )
        };
        if count == 0 || count > local.len() || count % 3 != 0 { return None; }
        output.truncate(count);
        reduced[lod] = output.into_iter().map(|i| gpu_ids[i as usize]).collect();
    }
    if reduced[0].len() == input.len() && reduced[1].len() == input.len() { return None; }
    Some((reduced, centre, radius))
}

/// The same projected-radius formula and post-clamp bias used by the regular
/// MD3 presenter. The input coordinates are renderer-space for both.
fn choose_lod(projected: f32, scale: f32, bias: i32) -> usize {
    let flod = if projected != 0.0 { 1.0 - projected * scale.clamp(0.0, 20.0) } else { 0.0 };
    let limit = 2isize;
    let lod = ((flod * 3.0) as isize).clamp(0, limit);
    (lod + bias.max(0) as isize).clamp(0, limit) as usize
}

/// The shared automatic-LOD gate wins over the BSP-only developer override.
/// The caller sends their conjunction as `enabled`.
fn choose_bsp_lod(enabled: bool, projected: f32, scale: f32, bias: i32) -> usize {
    if enabled { choose_lod(projected, scale, bias) } else { 0 }
}

pub(in crate::renderer) fn update(
    queue: &wgpu::Queue, world: &mut WorldGpu, camera: &Camera,
    width: u32, height: u32, scale: f32, bias: i32, debug: bool, enabled: bool,
) {
    let mut lod = std::mem::take(&mut world.bsp_model_lod);
    if lod.geometries.is_empty() { world.bsp_model_lod = lod; return; }
    let view = camera.position.to_array();
    let forward = camera.forward().to_array();
    // Legacy projected lights read independent immutable FULL-detail indices.
    // Translation and rotation both change projected depth. Do not let a
    // constant camera position freeze LOD across mouse movements.
    let key = (
        view.map(|v| (v / 16.0).floor() as i32),
        forward.map(|v| (v * 128.0).round() as i32),
        camera.cg_fov().round() as i32,
        scale.to_bits(), bias, debug, enabled,
    );
    let view_changed = lod.last_view != Some(key);
    lod.last_view = Some(key);
    let fov_y = camera.fov_y_for_viewport(width, height);
    let tan_half_fov = (fov_y * 0.5).tan().max(1.0e-6);
    let Some(source) = lod.original_indices.as_ref().cloned() else {
        world.bsp_model_lod = lod;
        return;
    };
    let mut scratch = Vec::new();
    let mut selected = [0usize; 3];
    let mut tris_full = 0usize;
    let mut tris_selected = 0usize;
    for geometry in &mut lod.geometries {
        if !view_changed { continue; }
        let mut changed = false;
        for part in &mut geometry.parts {
            if let Part::Model(model) = part {
                let depth = (0..3).map(|i| (model.centre[i] - view[i]) * forward[i]).sum::<f32>();
                // Parity with MD3's behind-camera/near-plane fallback.
                let projected = if depth <= 0.0 { 0.0 } else { model.radius / (depth * tan_half_fov) };
                let wanted = choose_bsp_lod(enabled, projected, scale, bias);
                selected[wanted] += 1;
                tris_full += (model.original.end - model.original.start) as usize / 3;
                tris_selected += if wanted == 0 {
                    (model.original.end - model.original.start) as usize / 3
                } else { model.reduced[wanted - 1].len() / 3 };
                if wanted != model.selected {
                    model.selected = wanted;
                    changed = true;
                }
            }
        }
        if !changed { continue; }
        geometry.version = geometry.version.wrapping_add(1);
        let has_lod = geometry.parts.iter().any(|part| matches!(part, Part::Model(m) if m.selected > 0));
        let range = if !has_lod {
            geometry.original.clone()
        } else {
            scratch.clear();
            for part in &geometry.parts {
                match part {
                    Part::Untouched(span) => scratch.extend_from_slice(&source[span.start as usize..span.end as usize]),
                    Part::Model(model) => {
                        if model.selected == 0 {
                            scratch.extend_from_slice(&source[model.original.start as usize..model.original.end as usize]);
                        } else {
                            scratch.extend_from_slice(&model.reduced[model.selected - 1]);
                        }
                    }
                }
            }
            debug_assert!(scratch.len() <= (geometry.reserved.end - geometry.reserved.start) as usize);
            queue.write_buffer(&world.index_buffer, u64::from(geometry.reserved.start) * 4, bytemuck::cast_slice(&scratch));
            geometry.reserved.start..geometry.reserved.start + scratch.len() as u32
        };
        if range == geometry.current { continue; }
        geometry.current = range.clone();
        for &(set, index) in &geometry.targets {
            let batches = if set == 0 { &mut world.coarse_batches } else { &mut world.full_batches };
            let batch = &mut batches[index];
            batch.indexed_range = range.clone();
            write_batch_range(queue, &world.cull_records_buffer, &world.indirect_buffer, batch);
            if set == 1 {
                if let Some(alias) = world.auto4_batches.get_mut(index) {
                    if index < world.auto4_variant_base {
                        alias.indexed_range = range.clone();
                        write_batch_range(queue, &world.cull_records_buffer, &world.indirect_buffer, alias);
                    }
                }
            }
        }
    }
    // The physical AUTO4 draw must reflect the same source-piece LOD as its
    // FULL fallback. Repack inside the existing allocated collapse range, with
    // no new batches, and preserve patch LOD's selections in mixed recipes.
    let changed_recipes = view_changed
        || lod.completed_recipes != world.auto4_collapse_cache.completed
        || world.patch_lod.stats().lod_updates != 0;
    if changed_recipes {
        lod.completed_recipes = world.auto4_collapse_cache.completed;
        // AUTO4 may keep previous clusters in its LRU cache; don't repack
        // non-visible cached geometries whenever the camera moves.
        let mut needed = vec![false; lod.recipe_members.len()];
        for &batch_id in &world.auto4_active_plan {
            if batch_id < world.auto4_variant_base { continue; }
            let variant = batch_id - world.auto4_variant_base;
            if let Some(&recipe) = world.auto4_variant_geometry.get(variant) {
                if let Some(slot) = needed.get_mut(recipe) { *slot = true; }
            }
        }
        for (recipe, &active) in needed.iter().enumerate() {
            if !active || lod.recipe_members[recipe].is_empty() { continue; }
            let Some(crate::renderer::Auto4GeometryState::Ready { range, .. }) =
                world.auto4_collapse_cache.states.get(recipe) else { continue; };
            let reserved = range.clone();
            let signature = lod.recipe_signature(recipe);
            if lod.recipe_reservations[recipe].as_ref() == Some(&reserved)
                && lod.recipe_versions[recipe] == signature
                && !(lod.recipe_has_patches[recipe] && world.patch_lod.stats().lod_updates != 0)
            { continue; }
            scratch.clear();
            for &member in &world.auto4_geometry_members[recipe] {
                let batch = &world.full_batches[member];
                if world.patch_lod.append_batch_indices(batch, &mut scratch) { continue; }
                if lod.append_batch_indices(&batch.full_indexed_range, &mut scratch) { continue; }
                scratch.extend_from_slice(&source[batch.full_indexed_range.start as usize..batch.full_indexed_range.end as usize]);
            }
            // Resident allocation is sized for the original, never a reduced
            // result. Patch LOD can only reduce it, too.
            if scratch.len() > (reserved.end - reserved.start) as usize {
                eprintln!("BSP MODEL LOD AUTO4: recipe {recipe} exceeded reserved index capacity; leaving existing draw intact");
                continue;
            }
            if !scratch.is_empty() {
                queue.write_buffer(&world.index_buffer, u64::from(reserved.start) * 4, bytemuck::cast_slice(&scratch));
            }
            let active = reserved.start..reserved.start + scratch.len() as u32;
            crate::renderer::visibility::compaction::auto4_set_geometry_range(queue, world, recipe, active);
            lod.recipe_versions[recipe] = signature;
            lod.recipe_reservations[recipe] = Some(reserved);
        }
    }
    if debug && view_changed {
        let state = (selected, tris_full, tris_selected);
        if lod.last_report != Some(state) {
            lod.last_report = Some(state);
            println!("BSP MODEL LOD SELECT: enabled={} lod0={} lod1={} lod2={} tris={}/{} (eligible pieces; includes AUTO4 resident recipes)",
                enabled, selected[0], selected[1], selected[2], tris_selected, tris_full);
        }
    }
    world.bsp_model_lod = lod;
}

fn write_batch_range(queue: &wgpu::Queue, records: &wgpu::Buffer, indirect: &wgpu::Buffer, batch: &WorldBatch) {
    let range = &batch.indexed_range;
    let draw = [range.end - range.start, range.start];
    queue.write_buffer(records,
        u64::from(batch.cull_index) * std::mem::size_of::<GpuCullRecord>() as u64
            + std::mem::offset_of!(GpuCullRecord, draw) as u64,
        bytemuck::cast_slice(&draw));
    let args = DrawIndexedIndirectArgs {
        index_count: draw[0], instance_count: 1, first_index: draw[1],
        base_vertex: 0, first_instance: 0,
    };
    queue.write_buffer(indirect,
        u64::from(batch.cull_index) * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
        bytemuck::bytes_of(&args));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn md3_lod_bias_and_scale_are_respected() {
        assert_eq!(choose_lod(0.5, 5.0, 0), 0);
        assert_eq!(choose_lod(0.01, 5.0, 0), 2);
        assert_eq!(choose_lod(0.5, 5.0, 5), 2);
        assert_eq!(choose_lod(0.5, 5.0, 0), 0);
    }

    #[test]
    fn shared_gate_restores_full_detail_even_with_forced_bias() {
        assert_eq!(choose_bsp_lod(true, 0.01, 5.0, 5), 2);
        assert_eq!(choose_bsp_lod(false, 0.01, 5.0, 5), 0);
        assert_eq!(choose_bsp_lod(true, 0.5, 5.0, 0), 0);
    }
}
