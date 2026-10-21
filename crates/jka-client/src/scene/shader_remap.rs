//! Runtime shader remapping (OpenJK/TaystJK `R_RemapShader`).
//!
//! Remaps are resolved on the cold mutation path. The resulting `PreparedMap`
//! contains ordinary direct draw batches, so there is no shader-name lookup or
//! remap branch in the renderer's per-frame/per-draw hot path.

use super::{
    material_batches::{append_material_batches, fuse_prepared_additive_batches, has_unfused_additive_batches},
    materials, AssetSearchPath, BTreeMap, DrawBatch,
    InlineModelGeometry, PreparedMap, PreparedRemapPlan, SurfaceMaterial, Textures,
};
use crate::materials::StageTexture;
use std::{path::Path, sync::Arc};

/// Canonical renderer shader name. `R_RemapShader` compares the old shader
/// after `COM_StripExtension`, while `R_FindShader` is slash/case insensitive.
pub(crate) fn canonical_shader_name(name: &str) -> String {
    let mut key = name.trim().replace('\\', "/").to_ascii_lowercase();
    let file_start = key.rfind('/').map_or(0, |slash| slash + 1);
    if let Some(dot) = key[file_start..].rfind('.') {
        key.truncate(file_start + dot);
    }
    key
}

/// Renderer-owned logical state corresponding to shader_t::remappedShader and
/// shader_t::timeOffset. Redirects are intentionally one hop: id Tech 3 swaps
/// the shader once when the draw surface is submitted; it does not recursively
/// chase remap chains.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ShaderRemapState {
    redirects: BTreeMap<String, String>,
    time_offsets: BTreeMap<String, f32>,
}

impl ShaderRemapState {
    pub(crate) fn clear(&mut self) {
        self.redirects.clear();
        self.time_offsets.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.redirects.is_empty() && self.time_offsets.is_empty()
    }

    /// Local and server-provided shader redirects for console diagnostics.
    /// Material hot paths consume already-resolved batches, not this iterator.
    pub(crate) fn redirects(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.redirects.iter().map(|(from, to)| (from.as_str(), to.as_str()))
    }

    /// Port of R_RemapShader's state mutation. `time_offset=None` is the local
    /// `/remapShader` command and therefore leaves the target shader's existing
    /// animation offset untouched. `old == new` clears only the redirect.
    pub(crate) fn apply(&mut self, old: &str, new: &str, time_offset: Option<f32>) -> bool {
        let old = canonical_shader_name(old);
        let new = canonical_shader_name(new);
        if old.is_empty() || new.is_empty() {
            return false;
        }
        let before_redirect = self.redirects.get(&old).cloned();
        let before_offset = self.time_offsets.get(&new).copied();
        if old == new {
            self.redirects.remove(&old);
        } else {
            self.redirects.insert(old.clone(), new.clone());
        }
        if let Some(offset) = time_offset.filter(|value| value.is_finite()) {
            self.time_offsets.insert(new.clone(), offset);
        }
        before_redirect != self.redirects.get(&old).cloned()
            || before_offset != self.time_offsets.get(&new).copied()
    }

    pub(crate) fn effective_name<'a>(&'a self, source: &'a str) -> std::borrow::Cow<'a, str> {
        if self.redirects.is_empty() {
            return std::borrow::Cow::Borrowed(source);
        }
        let key = canonical_shader_name(source);
        match self.redirects.get(&key) {
            Some(target) => std::borrow::Cow::Borrowed(target.as_str()),
            None if key == source => std::borrow::Cow::Borrowed(source),
            None => std::borrow::Cow::Owned(key),
        }
    }

    pub(crate) fn time_offset_for(&self, effective_shader: &str) -> f32 {
        self.time_offsets
            .get(&canonical_shader_name(effective_shader))
            .copied()
            .unwrap_or(0.0)
    }

    pub(crate) fn has_redirect_for(&self, source: &str) -> bool {
        self.redirects.contains_key(&canonical_shader_name(source))
    }
}

fn offset_index(index: &mut Option<usize>, base: usize) {
    if let Some(index) = index {
        *index += base;
    }
}

fn offset_material_textures(material: &mut SurfaceMaterial, base: usize) {
    for stage in &mut material.stages {
        if let StageTexture::Image(index) = &mut stage.texture {
            *index += base;
        }
        offset_index(&mut stage.enhancements.normal_texture, base);
        offset_index(&mut stage.enhancements.roughness_texture, base);
        offset_index(&mut stage.enhancements.height_texture, base);
        offset_index(&mut stage.enhancements.metallic_texture, base);
        offset_index(&mut stage.enhancements.specular_texture, base);
        offset_index(&mut stage.enhancements.emissive_texture, base);
    }
    if let Some(skybox) = &mut material.skybox {
        for texture in skybox {
            *texture += base;
        }
    }
    for sprite in &mut material.surface_sprite_effects {
        sprite.texture += base;
    }
}

fn same_material_geometry(a: &DrawBatch, b: &DrawBatch) -> bool {
    a.vertices == b.vertices
        && a.bsp_shader_index == b.bsp_shader_index
        && a.material_debug_index == b.material_debug_index
        && a.vertex_lit == b.vertex_lit
        && a.lightmap == b.lightmap
        && a.pvs_signature == b.pvs_signature
        && a.area_signature == b.area_signature
        && a.fog == b.fog
        && a.fog_is_global == b.fog_is_global
}

fn batch_shader_name<'a>(map: &'a PreparedMap, batch: &DrawBatch) -> Option<&'a str> {
    usize::try_from(batch.material_debug_index)
        .ok()
        .filter(|&index| index != u32::MAX as usize)
        .and_then(|index| map.material_debug.entries.get(index))
        .map(|entry| entry.name.as_str())
}

fn copy_geometry_specialization(source: &DrawBatch, replacement: &mut DrawBatch) {
    // These are geometric/map-position facts, not authored stage state. Preserve
    // them when the replacement shader opts into the corresponding feature.
    replacement.planar_plane = source.planar_plane;
    replacement.planar_pvs_origin = source.planar_pvs_origin;
    replacement.reflection_probe = source.reflection_probe;
    replacement.reflection_probe_position_radius = source.reflection_probe_position_radius;
    replacement.reflection_cache_flags = source.reflection_cache_flags;
    replacement.reflection_roughness_hint = source.reflection_roughness_hint;
    replacement.authored_ocean = source.authored_ocean;
}

fn rebuild_batches(
    map: &PreparedMap,
    source: &[DrawBatch],
    state: &ShaderRemapState,
    replacements: &BTreeMap<String, SurfaceMaterial>,
) -> Vec<DrawBatch> {
    let mut out = Vec::with_capacity(source.len());
    let mut index = 0usize;
    while index < source.len() {
        let first = &source[index];
        let mut end = index + 1;
        while end < source.len() && same_material_geometry(first, &source[end]) {
            end += 1;
        }

        let source_name = batch_shader_name(map, first);
        let effective = source_name.map(|name| state.effective_name(name));
        let redirected = source_name.is_some_and(|name| state.has_redirect_for(name));
        if redirected {
            let target = effective.as_deref().unwrap_or_default();
            if let Some(material) = replacements.get(target) {
                let start = out.len();
                append_material_batches(
                    &mut out,
                    material,
                    usize::try_from(first.bsp_shader_index).unwrap_or(usize::MAX),
                    usize::try_from(first.material_debug_index).ok(),
                    first.vertex_lit,
                    first.vertices.clone(),
                    first.lightmap,
                    &first.pvs_signature,
                    first.area_signature,
                    first.fog,
                    first.fog_is_global,
                );
                let time_offset = state.time_offset_for(target);
                for replacement in &mut out[start..] {
                    replacement.shader_time_offset = time_offset;
                    copy_geometry_specialization(first, replacement);
                }
            } else {
                // Tayst/OpenJK's R_RemapShader refuses a default/missing target
                // shader. Preserve the authored source material rather than
                // turning the surface white or dropping its draw entirely.
                let time_offset = source_name.map_or(0.0, |name| state.time_offset_for(name));
                out.extend(source[index..end].iter().cloned().map(|mut batch| {
                    batch.shader_time_offset = time_offset;
                    batch
                }));
            }
        } else {
            let time_offset = effective
                .as_deref()
                .map_or(0.0, |name| state.time_offset_for(name));
            out.extend(source[index..end].iter().cloned().map(|mut batch| {
                batch.shader_time_offset = time_offset;
                batch
            }));
        }
        index = end;
    }
    out
}

/// Rebuild stage lists separately for each inline model. A shader remap (or
/// reverse-lightmap fold) can change a material from N stages to M stages;
/// preserving old `InlineModelGeometry::batches` ranges would then index into
/// the wrong model, or past the end of the rebuilt batch vector during upload.
/// Keep the invariant that every model owns a contiguous, exact batch range.
fn rewrite_inline_model_batches<T>(
    source: &[T],
    models: &[InlineModelGeometry],
    mut rewrite: impl FnMut(&[T]) -> Vec<T>,
) -> (Vec<T>, Vec<InlineModelGeometry>) {
    let mut batches = Vec::with_capacity(source.len());
    let mut remapped_models = Vec::with_capacity(models.len());
    let mut previous_end = 0;
    for original in models {
        debug_assert_eq!(original.batches.start, previous_end);
        debug_assert!(original.batches.end <= source.len());
        let start = batches.len();
        batches.extend(rewrite(&source[original.batches.clone()]));
        let mut model = original.clone();
        model.batches = start..batches.len();
        remapped_models.push(model);
        previous_end = original.batches.end;
    }
    debug_assert_eq!(previous_end, source.len());
    (batches, remapped_models)
}

/// Expand the original authored additive stages retained inside each fused draw.
/// Unlike disabling blend work, this restores every stage (and its separate draw)
/// without reparsing shaders or BSP geometry. The prepared map cache remains fused.
pub(crate) fn expand_additive_batches(source: &[DrawBatch]) -> Vec<DrawBatch> {
    let mut out = Vec::with_capacity(source.iter().map(|b| 1 + b.additive_companions.len()).sum());
    for original in source {
        let mut primary = original.clone();
        primary.pipeline.additive_fusion_count = 0;
        primary.additive_companions.clear();
        out.push(primary);
        for companion in &original.additive_companions {
            let mut stage = companion.clone();
            stage.pipeline.additive_fusion_count = 0;
            stage.additive_companions.clear();
            out.push(stage);
        }
    }
    out
}

/// Reapply compatible additive stage fusion on a derived presentation map.
/// Handles a cache that was authored or uploaded with separate stage batches,
/// and is safe on a map whose stages are already fused. Since the batch count
/// can change, rebuild inline ranges and AUTO-4 portal lookup tables.
pub(crate) fn fuse_additive_stage_fusion_in_place(map: &mut PreparedMap) -> usize {
    // On the normal path stage fusion already ran on the map worker. Avoid
    // cloning world/PVS/inline batches unless a cold cache really needs repair.
    if !has_unfused_additive_batches(&map.batches)
        && !has_unfused_additive_batches(&map.pvs_batches)
        && !has_unfused_additive_batches(&map.inline_batches)
    {
        return 0;
    }
    let (coarse, saved_coarse) = fuse_prepared_additive_batches(&map.batches);
    let (pvs, saved_pvs) = fuse_prepared_additive_batches(&map.pvs_batches);
    let mut saved_inline = 0usize;
    let (inline, models) = rewrite_inline_model_batches(
        &map.inline_batches,
        &map.inline_models,
        |source| {
            let (draws, saved) = fuse_prepared_additive_batches(source);
            saved_inline += saved;
            draws
        },
    );
    let saved_total = saved_coarse + saved_pvs + saved_inline;
    if saved_total == 0 {
        return 0;
    }
    map.batches = coarse;
    map.pvs_batches = pvs;
    map.inline_batches = inline;
    map.inline_models = models;
    if let Some(visibility) = &map.visibility {
        map.portal_draw_plan = super::visibility::build_prepared_portal_draw_plan(
            &map.batches, &map.pvs_batches, visibility, &map.vertices, |_| {},
        );
    } else {
        map.portal_draw_plan = Default::default();
    }
    saved_total
}

/// Restore fully independent additive stages on the presentation copy only.
/// Inline model batch ranges and AUTO-4 portal indices must be rebuilt because
/// expanding 2/3-stage composites changes the number of stage batches.
pub(crate) fn expand_additive_stage_fusion_in_place(map: &mut PreparedMap) -> usize {
    let fused_count: usize = map.batches.iter()
        .chain(map.pvs_batches.iter())
        .chain(map.inline_batches.iter())
        .map(|b| b.additive_companions.len())
        .sum();
    if fused_count == 0 {
        return 0;
    }
    map.batches = expand_additive_batches(&map.batches);
    map.pvs_batches = expand_additive_batches(&map.pvs_batches);
    let (inline, models) = rewrite_inline_model_batches(
        &map.inline_batches,
        &map.inline_models,
        expand_additive_batches,
    );
    map.inline_batches = inline;
    map.inline_models = models;
    if let Some(visibility) = &map.visibility {
        map.portal_draw_plan = super::visibility::build_prepared_portal_draw_plan(
            &map.batches, &map.pvs_batches, visibility, &map.vertices, |_| {},
        );
    } else {
        map.portal_draw_plan = Default::default();
    }
    fused_count
}

fn resolve_remap_targets(
    map: &PreparedMap,
    root: &Path,
    game: Option<&Path>,
    options: super::MapPrepareOptions,
    state: &ShaderRemapState,
) -> Result<(BTreeMap<String, SurfaceMaterial>, Textures), String> {
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    assets.set_allow_asset_overrides(options.allow_asset_overrides);
    let mut textures = Textures::new();
    let texture_base = map.textures.len();
    let mut replacements = BTreeMap::<String, SurfaceMaterial>::new();

    for target in state.redirects.values() {
        if replacements.contains_key(target) {
            continue;
        }
        let definition = jka_assets::shader::find_shader(&map.shader_library, target);
        let origin_key = canonical_shader_name(target);
        let origin = map.shader_origins.get(&origin_key);
        let image_count_before = textures.images.len();
        let video_count_before = textures.videos.len();
        let mut material = materials::describe(
            target,
            0,
            definition,
            origin,
            &mut assets,
            &mut textures,
            options.gen_normal_maps,
            options.omit_environment_stages,
        );
        // R_RemapShader returns without changing anything when R_FindShader
        // produces the renderer's default shader. For an implicit shader in
        // this renderer, "no definition and no image/video was loaded" is the
        // equivalent failure signal. Explicit shader definitions remain valid
        // even when an individual authored stage has to use its normal fallback.
        let target_exists = definition.is_some()
            || textures.images.len() != image_count_before
            || textures.videos.len() != video_count_before;
        if !target_exists {
            textures.warnings.push(format!(
                "target {target} was not found; keeping the original shader"
            ));
            continue;
        }
        offset_material_textures(&mut material, texture_base);
        replacements.insert(target.clone(), material);
    }

    Ok((replacements, textures))
}

/// Re-resolve all currently active redirects into an already-prepared map.
/// The caller must supply an unremapped map: stage flattening is rebuilt from
/// its original batches so `old -> old` restores the authored material exactly.
pub(crate) fn apply_shader_remaps_in_place(
    map: &mut PreparedMap,
    root: &Path,
    game: Option<&Path>,
    options: super::MapPrepareOptions,
    state: &ShaderRemapState,
) -> Result<(), String> {
    if state.is_empty() {
        return Ok(());
    }

    let (replacements, mut textures) = resolve_remap_targets(map, root, game, options, state)?;
    let texture_base = map.textures.len();

    // Only these comparatively small flattened stage lists must be copied.
    // Geometry, collision, lightmaps and already-decoded textures stay in the
    // owned PreparedMap. This is also the path used before the first upload, so
    // an initial CS_SHADERSTATE never causes an unnecessary second world upload.
    let source_batches = map.batches.clone();
    let source_pvs_batches = map.pvs_batches.clone();
    let (inline_batches, inline_models) = rewrite_inline_model_batches(
        &map.inline_batches,
        &map.inline_models,
        |source| rebuild_batches(map, source, state, &replacements),
    );
    map.batches = rebuild_batches(map, &source_batches, state, &replacements);
    map.pvs_batches = rebuild_batches(map, &source_pvs_batches, state, &replacements);
    map.inline_batches = inline_batches;
    map.inline_models = inline_models;

    if let Some(visibility) = &map.visibility {
        map.portal_draw_plan = super::visibility::build_prepared_portal_draw_plan(
            &map.batches,
            &map.pvs_batches,
            visibility,
            &map.vertices,
            |_| {},
        );
    } else {
        map.portal_draw_plan = Default::default();
    }

    let hashes = textures.content_hashes();
    for video in &mut textures.videos {
        video.texture += texture_base;
    }
    Arc::make_mut(&mut map.textures).extend(textures.images);
    map.videos.extend(textures.videos);
    map.texture_hashes.extend(hashes);
    map.warnings.extend(
        textures
            .warnings
            .into_iter()
            .map(|warning| format!("shader remap: {warning}")),
    );
    Ok(())
}

/// Re-resolve all currently active redirects from the immutable, unremapped
/// restart cache. Runtime remaps use this cold path; ordinary rendering remains
/// identical to a normally prepared world and pays no remap lookup per draw.
pub(crate) fn apply_shader_remaps(
    base_map: &PreparedMap,
    root: &Path,
    game: Option<&Path>,
    options: super::MapPrepareOptions,
    state: &ShaderRemapState,
) -> Result<PreparedMap, String> {
    let mut map = base_map.clone();
    apply_shader_remaps_in_place(&mut map, root, game, options, state)?;
    Ok(map)
}

/// An ordinary material/texture substitution does not change the mapping
/// from FULL pieces to prepared AUTO4 geometry recipes. Keep those plans when
/// their existing merged members still share one new draw state; rebuilding
/// hundreds of camera-cluster plans just to change a texture is unnecessary.
fn can_reuse_portal_geometry(base: &PreparedMap, coarse: &[DrawBatch], full: &[DrawBatch]) -> bool {
    if base.batches.len() != coarse.len() || base.pvs_batches.len() != full.len() {
        return false;
    }
    if !base.batches.iter().zip(coarse).chain(base.pvs_batches.iter().zip(full))
        .all(|(old, next)| old.vertices == next.vertices
            && old.bsp_shader_index == next.bsp_shader_index
            && old.pipeline == next.pipeline
            && old.pvs_signature == next.pvs_signature
            && old.area_signature == next.area_signature)
    {
        return false;
    }
    base.portal_draw_plan.variants.iter().all(|variant| {
        full.get(variant.representative).is_some_and(|first|
            variant.members.iter().all(|&member|
                full.get(member).is_some_and(|piece|
                    super::draw_batches_share_state(first, piece))))
    })
}

/// Compile a cold-path remap into a *material-only* payload. The map's
/// collision, vertices, lightmaps, effects, ocean and PVS allocations stay in
/// the Arc-backed pristine cache and are never cloned for shader replacement.
/// The drawing hot path receives already-resolved pipeline/stage batches.
pub(crate) fn prepare_remap_plan(
    base: Arc<PreparedMap>,
    root: &Path,
    game: Option<&Path>,
    options: super::MapPrepareOptions,
    state: &ShaderRemapState,
    reverse_lightmap: bool,
    additive_fusion: bool,
) -> Result<PreparedRemapPlan, String> {
    let started = std::time::Instant::now();
    let (replacements, mut textures) = if state.is_empty() {
        (BTreeMap::new(), Textures::new())
    } else {
        resolve_remap_targets(&base, root, game, options, state)?
    };
    let mut coarse = rebuild_batches(&base, &base.batches, state, &replacements);
    let mut full = rebuild_batches(&base, &base.pvs_batches, state, &replacements);
    let (mut inline, mut models) = rewrite_inline_model_batches(
        &base.inline_batches, &base.inline_models,
        |batches| rebuild_batches(&base, batches, state, &replacements),
    );
    if reverse_lightmap {
        coarse = fold_reverse_lightmap_batches(&coarse).0;
        full = fold_reverse_lightmap_batches(&full).0;
        let rewritten = rewrite_inline_model_batches(&inline, &models,
            |batches| fold_reverse_lightmap_batches(batches).0);
        inline = rewritten.0;
        models = rewritten.1;
    }
    if additive_fusion {
        coarse = fuse_prepared_additive_batches(&coarse).0;
        full = fuse_prepared_additive_batches(&full).0;
        let rewritten = rewrite_inline_model_batches(&inline, &models,
            |batches| fuse_prepared_additive_batches(batches).0);
        inline = rewritten.0;
        models = rewritten.1;
    } else {
        coarse = expand_additive_batches(&coarse);
        full = expand_additive_batches(&full);
        let rewritten = rewrite_inline_model_batches(&inline, &models, expand_additive_batches);
        inline = rewritten.0;
        models = rewritten.1;
    }
    let reuse_portal = can_reuse_portal_geometry(&base, &coarse, &full);
    let portal_draw_plan = if reuse_portal {
        base.portal_draw_plan.clone()
    } else {
        base.visibility.as_ref().map_or_else(Default::default, |vis|
            super::build_prepared_portal_draw_plan(&coarse, &full, vis, &base.vertices, |_| {}))
    };
    let texture_hashes = textures.content_hashes();
    for video in &mut textures.videos {
        video.texture += base.textures.len();
    }
    rverbose!(1, "[SHADER REMAP] material-only CPU plan {:.2} ms; {} coarse / {} PVS / {} inline stages (no PreparedMap clone)",
        started.elapsed().as_secs_f64() * 1000.0, coarse.len(), full.len(), inline.len());
    if reuse_portal { rverbose!(2, "[SHADER REMAP] AUTO4 geometries reused; existing merged members remain compatible"); }
    Ok(PreparedRemapPlan {
        base,
        batches: coarse,
        pvs_batches: full,
        inline_batches: inline,
        inline_models: models,
        portal_draw_plan,
        replacement_textures: textures.images,
        replacement_texture_hashes: texture_hashes,
        replacement_videos: textures.videos,
        warnings: textures.warnings,
    })
}

/// Apply an opt-in OpenJK reverse-lightmap fold to a *derived* PreparedMap.
/// The restart cache must keep the pristine authored two-pass representation.
/// No texture parsing, BSP work or gameplay state is changed here.
pub(in crate::scene) fn fold_reverse_lightmap_batches(source: &[DrawBatch]) -> (Vec<DrawBatch>, usize) {
    let mut out = Vec::with_capacity(source.len());
    let mut folds = 0;
    let mut index = 0;
    while index < source.len() {
        let first = &source[index];
        let mut end = index + 1;
        while end < source.len() && same_material_geometry(first, &source[end]) {
            end += 1;
        }
        let group = &source[index..end];
        if group.len() >= 2
            && group[0].reverse_lightmap_fold_role == 1
            && group[1].reverse_lightmap_fold_role == 2
            && group[0].texture_is_lightmap
            && group[1].texture.is_some()
            && group[0].lightmap.is_some()
        {
            // Equivalent to stage_batch(material, second-without-blend,
            // first=true) in the existing folded forward-modulation path.
            let mut fused = group[1].clone();
            // Opaque pipelines are compiled without the alpha test; a folded
            // stage that still carries an alphaFunc cutoff stays Mask.
            fused.pipeline.class = if fused.alpha_cutoff != 0.0 {
                super::DrawClass::Mask
            } else {
                super::DrawClass::Opaque
            };
            fused.pipeline.blend = super::BlendMode::Opaque;
            fused.pipeline.depth_write = true;
            fused.modulate_lightmap = true;
            fused.detail_texture_eligible = fused.reverse_lightmap_fold_detail;
            fused.dlight_in_lightmap_stage = false;
            fused.fog_color_override = super::FogColorOverride::None;
            fused.reverse_lightmap_fold_role = 0;
            fused.reverse_lightmap_fold_detail = false;
            out.push(fused);
            for stage in &group[2..] {
                let mut stage = stage.clone();
                // The folded $lightmap pass no longer exists. As in the
                // ordinary append_material_batches wrapper, local dynamic
                // lighting must be evaluated by the first surviving stage.
                stage.dlight_in_lightmap_stage = false;
                stage.reverse_lightmap_fold_role = 0;
                out.push(stage);
            }
            folds += 1;
        } else {
            out.extend_from_slice(group);
        }
        index = end;
    }
    (out, folds)
}

pub(crate) fn apply_reverse_lightmap_collapse_in_place(map: &mut PreparedMap) -> usize {
    let (coarse, count) = fold_reverse_lightmap_batches(&map.batches);
    let (full, full_count) = fold_reverse_lightmap_batches(&map.pvs_batches);
    let mut inline_count = 0;
    let (inline, inline_models) = rewrite_inline_model_batches(
        &map.inline_batches,
        &map.inline_models,
        |source| {
            let (batches, count) = fold_reverse_lightmap_batches(source);
            inline_count += count;
            batches
        },
    );
    map.batches = coarse;
    map.pvs_batches = full;
    map.inline_batches = inline;
    map.inline_models = inline_models;
    // AUTO 4 plans refer to FULL indices. A recipe count change invalidates
    // their indices, so reconstruct the plan, not just its representative.
    if let Some(visibility) = &map.visibility {
        map.portal_draw_plan = super::build_prepared_portal_draw_plan(
            &map.batches, &map.pvs_batches, visibility, &map.vertices, |_| {},
        );
    } else {
        map.portal_draw_plan = Default::default();
    }
    count + full_count + inline_count
}

#[cfg(test)]
mod inline_batch_range_tests {
    use super::*;

    #[test]
    fn shader_remap_stage_count_changes_update_each_inline_model_range() {
        // Original models have five shader-stage batches and three batches.
        let models = [
            InlineModelGeometry { model: 1, vertices: 0..18, batches: 0..5 },
            InlineModelGeometry { model: 2, vertices: 18..30, batches: 5..8 },
        ];
        let source = [1, 2, 3, 4, 5, 6, 7, 8];
        // Five-stage fish material -> one-stage replacement; a later material
        // expands from three to six stages. Both changes must adjust offsets.
        let (batches, models) = rewrite_inline_model_batches(&source, &models, |old| {
            if old[0] == 1 { vec![10] } else { vec![20, 21, 22, 23, 24, 25] }
        });
        assert_eq!(batches, [10, 20, 21, 22, 23, 24, 25]);
        assert_eq!(models[0].batches, 0..1);
        assert_eq!(models[1].batches, 1..7);
        assert_eq!(models[0].vertices, 0..18);
        assert_eq!(models[1].vertices, 18..30);
        assert_eq!(models[0].model, 1);
        assert_eq!(models[1].model, 2);

        // A second rebuild (e.g. reverse-lightmap collapse) must use the
        // updated ranges and cannot revive the stale 0..5 / 5..8 offsets.
        let (folded, models) = rewrite_inline_model_batches(&batches, &models, |old| {
            if old[0] == 10 { old.to_vec() } else { vec![30, 31] }
        });
        assert_eq!(folded, [10, 30, 31]);
        assert_eq!(models[0].batches, 0..1);
        assert_eq!(models[1].batches, 1..3);
    }

    #[test]
    fn inline_range_rewrite_supports_empty_replacement_without_stale_indices() {
        let models = [
            InlineModelGeometry { model: 3, vertices: 0..3, batches: 0..2 },
            InlineModelGeometry { model: 4, vertices: 3..6, batches: 2..3 },
        ];
        let (batches, models) = rewrite_inline_model_batches(&[1, 2, 3], &models, |old| {
            if old[0] == 1 { vec![] } else { old.to_vec() }
        });
        assert_eq!(batches, [3]);
        assert_eq!(models[0].batches, 0..0);
        assert_eq!(models[1].batches, 0..1);
    }
}
