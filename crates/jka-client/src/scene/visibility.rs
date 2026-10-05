//! Visibility.
use crate::scene::{
    jka_position, BTreeMap, BTreeSet, DrawBatch, DrawClass, FoldHasher, GpuVertex, HashMap,
    PipelineKey, PreparedMap, PreparedPortalDrawPlan, PreparedPortalGeometry,
    PreparedPortalMergedVariant, PreparedPortalPlanBatchRef, Range, Visibility,
};

pub(in crate::scene) fn pvs_signature(vis: &Visibility, target_clusters: &[usize]) -> Vec<u64> {
    vis.pvs_signature(target_clusters)
}

pub(in crate::scene) fn portal_batch_visible_from_cluster(
    source: &DrawBatch,
    cluster: usize,
) -> bool {
    if source.pvs_signature.is_empty() {
        return true;
    }
    source
        .pvs_signature
        .get(cluster / 64)
        .is_some_and(|word| word & (1_u64 << (cluster % 64)) != 0)
}

/// Pieces AUTO 4 keeps as individual draws: promoted/authored water and planar
/// mirror stages are replaced or special-cased per piece by the renderer.
/// Everything else (including blended and sky stages) collapses within its
/// MINIMAL batch, which preserves MINIMAL's primitive order exactly.
pub(in crate::scene) fn auto4_collapsible(source: &DrawBatch) -> bool {
    !source.water
        && !source.water_primary
        && source.authored_ocean.is_none()
        && !source.planar_reflection
        && !source.planar_environment_candidate
}

/// Opaque/alpha-tested pieces are order-free under the depth test, so the
/// audit's ideal may merge them across MINIMAL batches by draw state.
pub(in crate::scene) fn auto4_merge_safe(source: &DrawBatch) -> bool {
    matches!(source.pipeline.class, DrawClass::Opaque | DrawClass::Mask)
        && auto4_collapsible(source)
}

/// For each FULL piece, the MINIMAL batch (same material stage, geometry
/// range containing the piece) that owns it. `None` keeps the piece as an
/// individual draw.
pub(in crate::scene) fn auto4_piece_owners(
    coarse: &[DrawBatch],
    full: &[DrawBatch],
) -> Vec<Option<usize>> {
    // All stages of one coarse geometry share one vertex range and are pushed
    // consecutively, in stage order.
    let mut order = (0..coarse.len()).collect::<Vec<_>>();
    order.sort_by_key(|&index| (coarse[index].vertices.start, index));
    let mut groups = Vec::<(Range<u32>, Vec<usize>)>::new();
    for index in order {
        let range = coarse[index].vertices.clone();
        if range.is_empty() {
            continue;
        }
        match groups.last_mut() {
            Some((last, stages)) if *last == range => stages.push(index),
            _ => groups.push((range, vec![index])),
        }
    }

    let mut owners = vec![None; full.len()];
    let mut stage = 0usize;
    for (index, piece) in full.iter().enumerate() {
        stage = if index > 0 && full[index - 1].vertices == piece.vertices {
            stage + 1
        } else {
            0
        };
        if piece.vertices.is_empty() {
            continue;
        }
        let Some(group) = groups
            .partition_point(|(range, _)| range.start <= piece.vertices.start)
            .checked_sub(1)
        else {
            continue;
        };
        let (range, stages) = &groups[group];
        if piece.vertices.end > range.end {
            continue;
        }
        owners[index] = stages
            .get(stage)
            .copied()
            .filter(|&owner| draw_batches_share_state(&coarse[owner], piece))
            .or_else(|| {
                stages
                    .iter()
                    .copied()
                    .find(|&owner| draw_batches_share_state(&coarse[owner], piece))
            });
    }
    owners
}

pub(in crate::scene) fn auto4_source_bounds(
    source: &DrawBatch,
    vertices: &[GpuVertex],
) -> ([f32; 3], [f32; 3]) {
    let start = usize::try_from(source.vertices.start).unwrap_or(usize::MAX);
    let end = usize::try_from(source.vertices.end)
        .unwrap_or(usize::MAX)
        .min(vertices.len());
    if start >= end {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for vertex in &vertices[start..end] {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(vertex.position[axis]);
            maximum[axis] = maximum[axis].max(vertex.position[axis]);
        }
    }
    (minimum, maximum)
}

pub(in crate::scene) fn auto4_union_bounds(
    a_min: [f32; 3],
    a_max: [f32; 3],
    b_min: [f32; 3],
    b_max: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let mut minimum = a_min;
    let mut maximum = a_max;
    for axis in 0..3 {
        minimum[axis] = minimum[axis].min(b_min[axis]);
        maximum[axis] = maximum[axis].max(b_max[axis]);
    }
    (minimum, maximum)
}

pub(crate) fn draw_batches_share_state(a: &DrawBatch, b: &DrawBatch) -> bool {
    // AUTO 4 and the renderer's multi-draw compactor must use one authoritative
    // compatibility test: a representative bind group/pipeline is only valid
    // when every draw-relevant field below matches.
    if a.planar_reflection
        || b.planar_reflection
        || a.planar_environment_candidate
        || b.planar_environment_candidate
    {
        return false;
    }
    a.pipeline == b.pipeline
        && a.texture == b.texture
        && a.texture_is_lightmap == b.texture_is_lightmap
        && a.texture_is_white == b.texture_is_white
        && a.normal_texture == b.normal_texture
        && a.roughness_texture == b.roughness_texture
        && a.height_texture == b.height_texture
        && a.metallic_texture == b.metallic_texture
        && a.specular_texture == b.specular_texture
        && a.emissive_texture == b.emissive_texture
        && a.height_from_alpha == b.height_from_alpha
        && a.rmo_packed == b.rmo_packed
        && a.rmo_specular_alpha == b.rmo_specular_alpha
        && a.normal_scale == b.normal_scale
        && a.roughness_override == b.roughness_override
        && a.specular_reflectance == b.specular_reflectance
        && a.parallax_depth == b.parallax_depth
        && a.lightmap == b.lightmap
        && a.modulate_lightmap == b.modulate_lightmap
        && a.dlight_in_lightmap_stage == b.dlight_in_lightmap_stage
        && a.vertex_lit == b.vertex_lit
        && a.tc_gen == b.tc_gen
        && a.tc_mods == b.tc_mods
        && a.rgb_gen == b.rgb_gen
        && a.alpha_gen == b.alpha_gen
        && a.color == b.color
        && a.alpha_cutoff == b.alpha_cutoff
        && a.fog == b.fog
        && a.fog_is_global == b.fog_is_global
        && a.fog_color_override == b.fog_color_override
        && a.legacy2_fog_in_stage_safe == b.legacy2_fog_in_stage_safe
        && a.global_fog_post_eligible == b.global_fog_post_eligible
        && a.skybox == b.skybox
}

/// Build AUTO 4's immutable cluster -> draw-recipe table while the map is still
/// on the CPU preparation path. For each camera cluster the exact PVS-visible
/// FULL pieces are regrouped under the MINIMAL batch that owns them, so a warm
/// plan issues exactly MINIMAL's draws, in MINIMAL's order, with only visible
/// geometry. Recipes are deduplicated globally; each unique member list maps to
/// one physical index recipe shared by every stage of the same surfaces.
pub(in crate::scene) fn build_prepared_portal_draw_plan<F>(
    coarse: &[DrawBatch],
    full: &[DrawBatch],
    visibility: &Visibility,
    vertices: &[GpuVertex],
    mut progress: F,
) -> PreparedPortalDrawPlan
where
    F: FnMut(u32),
{
    let cluster_count = visibility.clusters;
    if cluster_count == 0 || full.is_empty() {
        return PreparedPortalDrawPlan::default();
    }

    #[derive(Clone, Copy)]
    enum Slot {
        Base(usize),
        Owner(usize),
    }

    let owners = auto4_piece_owners(coarse, full);
    let bounds = full
        .iter()
        .map(|piece| auto4_source_bounds(piece, vertices))
        .collect::<Vec<_>>();
    let piece_len = |index: usize| {
        full[index]
            .vertices
            .end
            .saturating_sub(full[index].vertices.start) as usize
    };

    // Pieces visible from each cluster, ascending by piece index (the order the
    // per-cluster loop must see them in). Built by walking each piece's set bits
    // once instead of testing every piece against every cluster.
    let mut visible_pieces = vec![Vec::<u32>::new(); cluster_count];
    for (index, piece) in full.iter().enumerate() {
        let index = index as u32;
        if piece.pvs_signature.is_empty() {
            for list in &mut visible_pieces {
                list.push(index);
            }
            continue;
        }
        for (word_index, &word) in piece.pvs_signature.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let cluster = word_index * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if let Some(list) = visible_pieces.get_mut(cluster) {
                    list.push(index);
                }
            }
        }
    }

    type FastMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<FoldHasher>>;
    let mut members_by_owner = vec![Vec::<usize>::new(); coarse.len()];
    let mut slots = Vec::<Slot>::new();
    let mut variants = Vec::<PreparedPortalMergedVariant>::new();
    let mut variant_lookup = FastMap::<Vec<usize>, usize>::default();
    let mut geometries = Vec::<PreparedPortalGeometry>::new();
    let mut geometry_lookup = FastMap::<Vec<(u32, u32)>, usize>::default();
    let mut plans = Vec::<Vec<PreparedPortalPlanBatchRef>>::new();
    let mut plan_lookup = FastMap::<Vec<PreparedPortalPlanBatchRef>, usize>::default();
    let mut plan_by_cluster = Vec::with_capacity(cluster_count);
    let mut packed_index_count = 0usize;
    let mut reused_variant_hits = 0usize;
    let mut reused_plan_hits = 0usize;
    // Keep loading-screen traffic bounded on maps with hundreds/thousands of
    // clusters while still providing smooth enough progress feedback.
    let progress_stride = cluster_count.div_ceil(128).max(1);

    for cluster in 0..cluster_count {
        // Pieces are in MINIMAL submission order, so first-touch order of each
        // owner reproduces MINIMAL's draw order exactly.
        slots.clear();
        for &visible_index in &visible_pieces[cluster] {
            let index = visible_index as usize;
            let piece = &full[index];
            match owners[index] {
                Some(owner) if auto4_collapsible(piece) => {
                    let list = &mut members_by_owner[owner];
                    if list.is_empty() {
                        slots.push(Slot::Owner(owner));
                    }
                    list.push(index);
                }
                _ => slots.push(Slot::Base(index)),
            }
        }

        let mut refs = Vec::with_capacity(slots.len());
        for &slot in &slots {
            let owner = match slot {
                Slot::Base(index) => {
                    refs.push(PreparedPortalPlanBatchRef::Base(index));
                    continue;
                }
                Slot::Owner(owner) => owner,
            };
            let members = std::mem::take(&mut members_by_owner[owner]);
            if members.len() == 1 {
                refs.push(PreparedPortalPlanBatchRef::Base(members[0]));
                continue;
            }
            if let Some(&variant) = variant_lookup.get(&members) {
                reused_variant_hits += 1;
                refs.push(PreparedPortalPlanBatchRef::Merged(variant));
                continue;
            }
            let ranges = members
                .iter()
                .map(|&member| (full[member].vertices.start, full[member].vertices.end))
                .collect::<Vec<_>>();
            let geometry = *geometry_lookup.entry(ranges).or_insert_with(|| {
                let contiguous = members
                    .windows(2)
                    .all(|pair| full[pair[0]].vertices.end == full[pair[1]].vertices.start);
                if !contiguous {
                    packed_index_count += members
                        .iter()
                        .map(|&member| piece_len(member))
                        .sum::<usize>();
                }
                geometries.push(PreparedPortalGeometry {
                    members: members.clone(),
                    contiguous,
                });
                geometries.len() - 1
            });
            let first_area = full[members[0]].area_signature;
            let mut area_signature = [0_u64; 4];
            let mut area_mixed = false;
            let (mut bounds_min, mut bounds_max) = bounds[members[0]];
            for &member in &members {
                let signature = full[member].area_signature;
                area_mixed |= signature != first_area;
                for (word, value) in area_signature.iter_mut().zip(signature) {
                    *word |= value;
                }
                (bounds_min, bounds_max) =
                    auto4_union_bounds(bounds_min, bounds_max, bounds[member].0, bounds[member].1);
            }
            let variant = variants.len();
            variants.push(PreparedPortalMergedVariant {
                representative: members[0],
                members: members.clone(),
                geometry,
                area_signature,
                area_mixed,
                bounds_min,
                bounds_max,
            });
            variant_lookup.insert(members, variant);
            refs.push(PreparedPortalPlanBatchRef::Merged(variant));
        }

        let plan_id = if let Some(&plan_id) = plan_lookup.get(&refs) {
            reused_plan_hits += 1;
            plan_id
        } else {
            plans.push(refs.clone());
            plan_lookup.insert(refs, plans.len() - 1);
            plans.len() - 1
        };
        plan_by_cluster.push(plan_id);

        let completed = cluster + 1;
        if completed == cluster_count || completed % progress_stride == 0 {
            progress(u32::try_from(completed).unwrap_or(u32::MAX));
        }
    }

    PreparedPortalDrawPlan {
        variants,
        geometries,
        plans,
        plan_by_cluster,
        piece_count: full.len(),
        packed_index_count,
        reused_variant_hits,
        reused_plan_hits,
    }
}

/// Offline AUTO 4 audit for `--validate-map`. For every camera cluster it
/// compares MINIMAL, FULL, the current AUTO 4 recipe and the ideal recipe
/// (exact PVS-visible FULL pieces rebatched purely by draw-state class), and
/// attributes every AUTO 4 draw above the ideal to the rule that caused it.
pub fn auto4_audit_lines(map: &PreparedMap) -> Vec<String> {
    let plan = &map.portal_draw_plan;
    let cluster_count = plan.plan_by_cluster.len();
    if cluster_count == 0 {
        return vec!["AUTO 4 audit: no compiled PVS / no AUTO 4 plan".into()];
    }
    let tris =
        |batch: &DrawBatch| u64::from(batch.vertices.end.saturating_sub(batch.vertices.start) / 3);

    // Global draw-state class ids, shared by FULL and portal pieces so "ideal"
    // and "current" are measured against the renderer's own compatibility key.
    fn classify(batch: &DrawBatch, reps: &mut Vec<DrawBatch>) -> usize {
        if let Some(id) = reps
            .iter()
            .position(|rep| draw_batches_share_state(rep, batch))
        {
            id
        } else {
            reps.push(batch.clone());
            reps.len() - 1
        }
    }
    let mut reps = Vec::<DrawBatch>::new();
    let full_class = map
        .pvs_batches
        .iter()
        .map(|b| classify(b, &mut reps))
        .collect::<Vec<_>>();
    let class_count = reps.len();
    // Relaxed keys for "what if the binding model changed": lightmap page moved
    // into a texture array (lightmap ignored), and fully bindless materials
    // (only the fixed-function PipelineKey remains).
    let mut reps_nolm = Vec::<DrawBatch>::new();
    let full_class_nolm = map
        .pvs_batches
        .iter()
        .map(|b| {
            let mut probe = b.clone();
            probe.lightmap = None;
            classify(&probe, &mut reps_nolm)
        })
        .collect::<Vec<_>>();
    let mut pipelines = Vec::<PipelineKey>::new();
    let full_pipeline = map
        .pvs_batches
        .iter()
        .map(|b| {
            pipelines
                .iter()
                .position(|p| *p == b.pipeline)
                .unwrap_or_else(|| {
                    pipelines.push(b.pipeline);
                    pipelines.len() - 1
                })
        })
        .collect::<Vec<_>>();

    #[derive(Default, Clone, Copy)]
    struct Sum {
        draws: u64,
        tris: u64,
        max_draws: u64,
        max_tris: u64,
    }
    impl Sum {
        fn add(&mut self, draws: u64, tris: u64) {
            self.draws += draws;
            self.tris += tris;
            self.max_draws = self.max_draws.max(draws);
            self.max_tris = self.max_tris.max(tris);
        }
    }
    let mut minimal = Sum::default();
    let mut minimal_mergeable = 0u64;
    let mut full = Sum::default();
    let mut auto4_warm = Sum::default();
    let mut auto4_cold = Sum::default();
    let mut ideal = Sum::default();
    let mut ideal_nolm = 0u64;
    let mut ideal_bindless = 0u64;
    let (mut over_minimal, mut over_ideal) = (0u64, 0u64);
    // Ideal recipe memory: unique (class, visible FULL piece set) pairs that
    // are not "everything in this class", and how many are one contiguous run.
    let mut ideal_recipes = BTreeSet::<(usize, Vec<usize>)>::new();
    let class_full_members = {
        let mut members = vec![Vec::<usize>::new(); class_count];
        for (index, &class) in full_class.iter().enumerate() {
            if auto4_merge_safe(&map.pvs_batches[index]) {
                members[class].push(index);
            }
        }
        members
    };
    let mut worst = (0u64, 0usize);
    let mut rows = Vec::<(usize, [u64; 11])>::new();

    for cluster in 0..cluster_count {
        let visible = |b: &DrawBatch| portal_batch_visible_from_cluster(b, cluster);
        // MINIMAL: every coarse batch with any visible piece, full index range.
        let (mut min_draws, mut min_tris, mut min_unmergeable) = (0u64, 0u64, 0u64);
        for batch in map.batches.iter().filter(|b| visible(b)) {
            min_draws += 1;
            min_tris += tris(batch);
            if !auto4_merge_safe(batch) {
                min_unmergeable += 1;
            }
        }
        minimal.add(min_draws, min_tris);
        minimal_mergeable += min_draws - min_unmergeable;

        // FULL + IDEAL. IDEAL = exact visible FULL pieces; opaque/mask merged by
        // draw-state class (order-free under depth test), everything else kept
        // at MINIMAL's one-draw-per-coarse-batch order with only visible pieces.
        let mut full_by_class = vec![Vec::<usize>::new(); class_count];
        let mut nolm = BTreeSet::<usize>::new();
        let mut bindless = BTreeSet::<usize>::new();
        let (mut full_draws, mut full_tris) = (0u64, 0u64);
        for (index, batch) in map
            .pvs_batches
            .iter()
            .enumerate()
            .filter(|(_, b)| visible(b))
        {
            full_draws += 1;
            full_tris += tris(batch);
            if auto4_merge_safe(batch) {
                full_by_class[full_class[index]].push(index);
                nolm.insert(full_class_nolm[index]);
                bindless.insert(full_pipeline[index]);
            }
        }
        full.add(full_draws, full_tris);
        let merge_classes = full_by_class.iter().filter(|m| !m.is_empty()).count() as u64;
        let ideal_draws = merge_classes + min_unmergeable;
        ideal.add(ideal_draws, full_tris);
        ideal_nolm += nolm.len() as u64 + min_unmergeable;
        ideal_bindless += bindless.len() as u64 + min_unmergeable;
        for (class, members) in full_by_class.into_iter().enumerate() {
            if !members.is_empty() && members != class_full_members[class] {
                ideal_recipes.insert((class, members));
            }
        }

        // Current AUTO 4 recipe for this cluster. Warm = one draw per ref;
        // cold = members drawn individually until a non-contiguous recipe's
        // physical index range has been built.
        let refs = &plan.plans[plan.plan_by_cluster[cluster]];
        let (mut warm, mut cold, mut a4_tris) = (0u64, 0u64, 0u64);
        for reference in refs {
            warm += 1;
            let members: &[usize] = match reference {
                PreparedPortalPlanBatchRef::Base(index) => std::slice::from_ref(index),
                PreparedPortalPlanBatchRef::Merged(variant) => &plan.variants[*variant].members,
            };
            cold += match reference {
                PreparedPortalPlanBatchRef::Merged(variant)
                    if !plan.geometries[plan.variants[*variant].geometry].contiguous =>
                {
                    members.len() as u64
                }
                _ => 1,
            };
            for &member in members {
                a4_tris += tris(&map.pvs_batches[member]);
            }
        }
        auto4_warm.add(warm, a4_tris);
        auto4_cold.add(cold, a4_tris);
        let row_over_minimal = warm.saturating_sub(min_draws);
        let row_over_ideal = warm.saturating_sub(ideal_draws);
        over_minimal += row_over_minimal;
        over_ideal += row_over_ideal;
        let excess = row_over_ideal;
        if excess > worst.0 {
            worst = (excess, cluster);
        }
        rows.push((
            cluster,
            [
                min_draws,
                min_tris,
                full_draws,
                full_tris,
                warm,
                cold,
                a4_tris,
                ideal_draws,
                row_over_minimal,
                row_over_ideal,
                0,
            ],
        ));
    }

    let n = cluster_count as f64;
    let mut lines = Vec::new();
    lines.push(format!(
        "AUTO 4 audit: {cluster_count} cluster(s), {class_count} draw-state class(es) ({} ignoring lightmap page, {} fixed-function pipelines); {} coarse / {} full batch(es); {} recipe(s) over {} geometr(ies), {} contiguous",
        reps_nolm.len(),
        pipelines.len(),
        map.batches.len(),
        map.pvs_batches.len(),
        plan.variants.len(),
        plan.geometries.len(),
        plan.geometries.iter().filter(|geometry| geometry.contiguous).count(),
    ));
    for (label, sum) in [
        ("MINIMAL", &minimal),
        ("FULL", &full),
        ("AUTO 4 warm", &auto4_warm),
        ("AUTO 4 cold", &auto4_cold),
        ("IDEAL (current key)", &ideal),
    ] {
        lines.push(format!(
            "  {label:<20} avg {:7.1} draws / {:9.0} tris   max {:5} draws / {:8} tris",
            sum.draws as f64 / n,
            sum.tris as f64 / n,
            sum.max_draws,
            sum.max_tris
        ));
    }
    lines.push(format!(
        "  MINIMAL draw mix avg: {:.1} opaque/mask, {:.1} transparent/sky/water/env",
        minimal_mergeable as f64 / n,
        (minimal.draws - minimal_mergeable) as f64 / n,
    ));
    lines.push(format!(
        "  IDEAL with lightmap texture-array avg {:.1} draws; with bindless materials avg {:.1} draws",
        ideal_nolm as f64 / n,
        ideal_bindless as f64 / n,
    ));
    lines.push(format!(
        "  AUTO 4 warm draws above MINIMAL avg {:.2}, above IDEAL avg {:.2} per cluster",
        over_minimal as f64 / n,
        over_ideal as f64 / n,
    ));
    let recipe_indices: u64 = ideal_recipes
        .iter()
        .map(|(_, members)| {
            members
                .iter()
                .map(|&m| tris(&map.pvs_batches[m]) * 3)
                .sum::<u64>()
        })
        .sum();
    let single_run = ideal_recipes
        .iter()
        .filter(|(_, members)| {
            members
                .windows(2)
                .all(|w| map.pvs_batches[w[0]].vertices.end == map.pvs_batches[w[1]].vertices.start)
        })
        .count();
    lines.push(format!(
        "  IDEAL recipes needing new index data: {} unique ({} already one contiguous vertex run), eager cost {:.2} MiB",
        ideal_recipes.len(),
        single_run,
        recipe_indices as f64 * 4.0 / (1024.0 * 1024.0)
    ));
    rows.sort_by_key(|(_, r)| r[3]);
    let median = rows[rows.len() / 2];
    let heaviest = *rows.last().unwrap();
    let worst_row = *rows.iter().find(|(c, _)| *c == worst.1).unwrap_or(&median);
    for (label, (cluster, r)) in [
        ("median", median),
        ("heaviest", heaviest),
        ("worst-excess", worst_row),
    ] {
        lines.push(format!(
            "  {label:<12} cluster {cluster:5}: MINIMAL {}d/{}t  FULL {}d/{}t  AUTO4 warm {}d (cold {}d)/{}t  IDEAL {}d/{}t  above MINIMAL {}, above IDEAL {}",
            r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[3], r[8], r[9]
        ));
    }
    // Strict plan verification: every cluster must cover exactly its visible
    // FULL pieces once, and every recipe's bounds must contain its pieces.
    let piece_bounds = map
        .pvs_batches
        .iter()
        .map(|piece| auto4_source_bounds(piece, &map.vertices))
        .collect::<Vec<_>>();
    let mut coverage_errors = 0usize;
    let mut first_coverage_error = None;
    for cluster in 0..cluster_count {
        let mut expected = (0..plan.piece_count.min(map.pvs_batches.len()))
            .filter(|&index| portal_batch_visible_from_cluster(&map.pvs_batches[index], cluster))
            .collect::<Vec<_>>();
        let mut covered = Vec::new();
        for reference in &plan.plans[plan.plan_by_cluster[cluster]] {
            match reference {
                PreparedPortalPlanBatchRef::Base(index) => covered.push(*index),
                PreparedPortalPlanBatchRef::Merged(variant) => {
                    covered.extend_from_slice(&plan.variants[*variant].members)
                }
            }
        }
        expected.sort_unstable();
        covered.sort_unstable();
        if expected != covered {
            coverage_errors += 1;
            first_coverage_error.get_or_insert((cluster, expected.len(), covered.len()));
        }
    }
    let mut bounds_errors = 0usize;
    for variant in &plan.variants {
        let contains = |member: usize| {
            let (minimum, maximum) = piece_bounds[member];
            let empty = map.pvs_batches[member].vertices.is_empty();
            empty
                || (0..3).all(|axis| {
                    minimum[axis] >= variant.bounds_min[axis]
                        && maximum[axis] <= variant.bounds_max[axis]
                })
        };
        if !variant.members.iter().all(|&member| contains(member)) {
            bounds_errors += 1;
        }
    }
    lines.push(format!(
        "  VERIFY: {coverage_errors} cluster(s) with wrong piece coverage{}, {bounds_errors} recipe(s) with bounds not containing their pieces",
        first_coverage_error
            .map(|(cluster, expected, covered)| format!(" (first: cluster {cluster}, expected {expected} pieces, plan covers {covered})"))
            .unwrap_or_default(),
    ));
    // JKA_AUDIT_POS="x y z" in renderer coordinates (as printed by
    // [JKA PERF STATE] camera=[..]) reports that camera's cluster and plan.
    if let (Some(position), Some(vis)) =
        (std::env::var("JKA_AUDIT_POS").ok(), map.visibility.as_ref())
    {
        let parts = position
            .split(|c: char| c == ' ' || c == ',')
            .filter_map(|part| part.trim().parse::<f32>().ok())
            .collect::<Vec<_>>();
        if let [x, y, z] = parts[..] {
            match vis.cluster_at(jka_position([x, y, z])) {
                Some(cluster) if cluster < cluster_count => {
                    let plan_id = plan.plan_by_cluster[cluster];
                    let refs = &plan.plans[plan_id];
                    let visible = map
                        .pvs_batches
                        .iter()
                        .filter(|piece| portal_batch_visible_from_cluster(piece, cluster))
                        .count();
                    lines.push(format!(
                        "  POS [{x}, {y}, {z}] -> cluster {cluster}, plan {plan_id}: {} ref(s), {visible} visible FULL piece(s)",
                        refs.len()
                    ));
                    for reference in refs {
                        if let PreparedPortalPlanBatchRef::Merged(variant) = reference {
                            let recipe = &plan.variants[*variant];
                            let geometry = &plan.geometries[recipe.geometry];
                            lines.push(format!(
                                "    recipe {variant}: {} member(s) {:?}, geometry {} contiguous={} area_mixed={} bounds {:?}..{:?}",
                                recipe.members.len(),
                                &recipe.members[..recipe.members.len().min(8)],
                                recipe.geometry,
                                geometry.contiguous,
                                recipe.area_mixed,
                                recipe.bounds_min,
                                recipe.bounds_max,
                            ));
                        }
                    }
                }
                other => lines.push(format!(
                    "  POS [{x}, {y}, {z}] -> cluster {other:?} (outside PVS)"
                )),
            }
        }
    }
    lines
}

/// Order one MINIMAL batch's PVS pieces so pieces seen from the same camera
/// clusters are adjacent. AUTO 4 draws each cluster's visible pieces of a batch
/// as one indexed draw; when those pieces are adjacent, FULL's index data
/// already holds that draw as one run and nothing has to be built at runtime.
/// The sum of Hamming distances between neighbouring signatures counts the
/// visibility-run boundaries over all clusters, so a greedy nearest-neighbour
/// path over that distance, refined with 2-opt, keeps each cluster's visible
/// pieces together.
pub(in crate::scene) fn order_pvs_pieces<T>(
    pieces: BTreeMap<(Vec<u64>, [u64; 4]), T>,
) -> Vec<((Vec<u64>, [u64; 4]), T)> {
    // Quadratic; very large batches keep signature order to bound load time.
    const MAX_ORDERED_PIECES: usize = 4096;
    let mut pieces = pieces.into_iter().collect::<Vec<_>>();
    let count = pieces.len();
    if count <= 2 || count > MAX_ORDERED_PIECES {
        return pieces;
    }
    // Signatures in one contiguous matrix, zero-padded to a common width. A
    // missing word counts as zero, so xor with the padding equals the old
    // "popcount of the longer tail" and every distance is unchanged.
    let stride = pieces
        .iter()
        .map(|piece| piece.0 .0.len())
        .max()
        .unwrap_or(0);
    let mut matrix = vec![0_u64; count * stride];
    for (index, piece) in pieces.iter().enumerate() {
        matrix[index * stride..index * stride + piece.0 .0.len()].copy_from_slice(&piece.0 .0);
    }
    let signature = |piece: usize| &matrix[piece * stride..(piece + 1) * stride];
    let distance = |a: &[u64], b: &[u64]| -> u32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x ^ y).count_ones())
            .sum::<u32>()
    };
    // Start from the most narrowly visible piece: it is a natural path end.
    let mut current = (0..count)
        .min_by_key(|&index| {
            signature(index)
                .iter()
                .map(|word| word.count_ones())
                .sum::<u32>()
        })
        .unwrap_or(0);
    let mut visited = vec![false; count];
    let mut order = Vec::with_capacity(count);
    visited[current] = true;
    order.push(current);
    for _ in 1..count {
        let next = (0..count)
            .filter(|&index| !visited[index])
            .min_by_key(|&index| distance(signature(current), signature(index)))
            .expect("unvisited piece remains");
        visited[next] = true;
        order.push(next);
        current = next;
    }
    // 2-opt: reverse any segment whose endpoints then join more cheaply.
    for _ in 0..8 {
        let mut improved = false;
        for i in 0..count.saturating_sub(2) {
            for j in i + 2..count {
                let (a, b, c) = (order[i], order[i + 1], order[j]);
                let d = order.get(j + 1).copied();
                let before = distance(signature(a), signature(b))
                    + d.map_or(0, |d| distance(signature(c), signature(d)));
                let after = distance(signature(a), signature(c))
                    + d.map_or(0, |d| distance(signature(b), signature(d)));
                if after < before {
                    order[i + 1..=j].reverse();
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    let mut slots = pieces.drain(..).map(Some).collect::<Vec<_>>();
    order
        .into_iter()
        .map(|index| slots[index].take().expect("each piece is ordered once"))
        .collect()
}
