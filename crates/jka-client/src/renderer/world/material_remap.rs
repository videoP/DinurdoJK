//! Live R_RemapShader for resident BSP geometry.
//!
//! TaystJK changes shader_t::remappedShader instead of rebuilding a BSP. WGPU
//! has explicit material bind groups, so the cold command path regenerates the
//! *stage tables* and indirect/cull descriptors, but retains all resident world
//! geometry, lightmaps, grass, lighting, AO and ocean simulation state. Nothing
//! in the ordinary frame/draw path consults the shader-name remap table.
use crate::renderer::{
    build_draw_compaction_groups, build_world_animation, create_cull_debug_bind_group,
    create_fast_world_bind_group, create_gpu_cull_bind_group, create_world_bind_group,
    instantiate_portal_draw_plans, material_uniform,
    visible_batches_by_cluster, weather, Arc, Auto4CollapseCache, Auto4CollapseWorker,
    Auto4GeometryState, DrawBatch, DrawClass, DrawIndexedIndirectArgs, GpuCullRecord,
    MaterialFragmentKey, Renderer, UniformArena, UniformSlot,
    WorldAnimationFrame, WorldBatch, WorldGpu, WorldUploadTimings, AUTO_DETAIL_ENABLED_BIT,
    PLANAR_REFLECTION_SLOTS,
};
use std::{collections::{BTreeMap, BTreeSet}, ops::Range};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

/// The original indexed triangles, not the number/order of shader passes, are
/// the identity of a BSP draw. Inline indices use a separate source buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct GeometryKey {
    start: u32,
    end: u32,
    inline: bool,
}

fn geometry_key(source: &DrawBatch, inline: bool) -> GeometryKey {
    GeometryKey { start: source.vertices.start, end: source.vertices.end, inline }
}

/// Check that a newly flattened stage plan can be represented by the current
/// immutable index buffers before mutating ANY renderer-owned resources.
fn resident_geometry<'a>(
    world: &'a WorldGpu,
    map: &crate::scene::PreparedRemapPlan,
) -> Result<BTreeMap<GeometryKey, Vec<&'a WorldBatch>>, String> {
    let mut geometry = BTreeMap::new();
    for batch in world.coarse_batches.iter().chain(world.full_batches.iter()) {
        let mut key = geometry_key(&batch.source, batch.is_inline_entity);
        if key.inline {
            // Upload shifts inline source vertices beyond the world source
            // range for inspector picking. Undo that shift for cold lookup.
            let shift = map.base.vertices.len() as u32;
            key.start = key.start.saturating_sub(shift);
            key.end = key.end.saturating_sub(shift);
        }
        geometry.entry(key).or_insert_with(Vec::new).push(batch);
    }
    for (sources, inline) in [(&map.batches, false), (&map.pvs_batches, false), (&map.inline_batches, true)] {
        for source in sources {
            let key = geometry_key(source, inline);
            if !geometry.contains_key(&key) {
                return Err(format!("material {}/{} needs geometry that is not resident", key.start, key.end));
            }
        }
    }
    Ok(geometry)
}

/// Cold mutation-path proof that the existing GPU cull/indirect/AUTO4
/// descriptor layout can be retained exactly. New materials may use different
/// bindings, but neither draw geometry, pipeline classification nor the
/// compatibility of *existing* compact groups may change.
fn compatible_live_stage_layout(
    world: &WorldGpu,
    map: &crate::scene::PreparedRemapPlan,
    coarse: &[WorldBatch],
    full: &[WorldBatch],
) -> bool {
    if world.coarse_batches.len() != coarse.len() || world.full_batches.len() != full.len()
        || world.auto4_batches.len() != world.auto4_variant_base + map.portal_draw_plan.variants.len()
        || world.primary_skybox != map.batches.iter().chain(&map.inline_batches).find_map(|b| b.skybox)
        || world.auto4_plan_refs != map.portal_draw_plan.plans
        || world.auto4_plan_by_cluster != map.portal_draw_plan.plan_by_cluster
        || world.auto4_variant_members.len() != map.portal_draw_plan.variants.len()
        || world.auto4_geometry_members.len() != map.portal_draw_plan.geometries.len()
    {
        return false;
    }
    for (old, next) in world.coarse_batches.iter().zip(coarse)
        .chain(world.full_batches.iter().zip(full))
    {
        if old.source.vertices != next.source.vertices
            || old.source.bsp_shader_index != next.source.bsp_shader_index
            || old.source.material_debug_index != next.source.material_debug_index
            || old.source.pipeline != next.source.pipeline
            || old.source.surface_material != next.source.surface_material
            || old.source.water != next.source.water
            || old.source.glass_primary != next.source.glass_primary
            || (old.source.water && !crate::renderer::same_draw_state(&old.source, &next.source))
            || old.source.skybox != next.source.skybox
            || old.source.authored_glow != next.source.authored_glow
            || old.source.planar_reflection != next.source.planar_reflection
            || old.source.planar_environment_candidate != next.source.planar_environment_candidate
            || old.full_indexed_range != next.full_indexed_range
            || old.inline_model_slot != next.inline_model_slot
            || old.is_inline_entity != next.is_inline_entity
        {
            return false;
        }
    }
    for (old, next) in world.auto4_variant_members.iter().zip(&map.portal_draw_plan.variants) {
        if old != &next.members { return false; }
        if next.members.iter().skip(1).any(|&i| !crate::renderer::same_draw_state(
            &full[next.representative].source, &full[i].source)) { return false; }
    }
    for (old, next) in world.auto4_geometry_members.iter().zip(&map.portal_draw_plan.geometries) {
        if old != &next.members { return false; }
    }
    // Any existing multi-draw compaction group must remain homogeneous after
    // changing stage bindings. New merge opportunities can wait until the
    // next map load; they do not change the number of draws in this frame.
    let mut compact_representatives = BTreeMap::<u32, &DrawBatch>::new();
    for (old, batch) in world.coarse_batches.iter().zip(coarse)
        .chain(world.full_batches.iter().zip(full)) {
        if let Some(group) = old.compact_group {
            if let Some(representative) = compact_representatives.get(&group) {
                if !crate::renderer::same_draw_state(representative, &batch.source) { return false; }
            } else {
                compact_representatives.insert(group, &batch.source);
            }
        }
    }
    true
}

/// Replace shader properties only. Never touch the physical index range,
/// face-cull runs, cull index, LOD ownership, compact group or inline slot.
fn replace_stage_material(dst: &mut WorldBatch, src: &WorldBatch) {
    dst.source = src.source.clone();
    dst.fragment_key = src.fragment_key;
    dst.bind_group = src.bind_group.clone();
    dst.fast_bind_group = src.fast_bind_group.clone();
    dst.authored_glow_bind_group = src.authored_glow_bind_group.clone();
    dst.animation = src.animation.clone();
    dst._material_buffer = src._material_buffer.clone();
    dst.additive_uniforms = src.additive_uniforms.clone();
    dst._fog_buffer = src._fog_buffer.clone();
    dst._authored_glow_fog_buffer = src._authored_glow_fog_buffer.clone();
}

/// A changed shader pass count must repack dense cull records, but does not
/// imply reallocating GPU storage. Keeping buffer capacity (and rewriting only
/// used bytes) avoids repeated driver allocation on multipass->single-pass
/// remaps. This function runs exclusively while processing a remap command.
fn update_or_grow_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    resident: &wgpu::Buffer,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    if resident.size() >= contents.len().max(4) as u64 {
        if !contents.is_empty() { queue.write_buffer(resident, 0, contents); }
        resident.clone()
    } else {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label), contents, usage,
        })
    }
}

impl Renderer {
    /// Rebuilds only GPU material/stage metadata. Does not call build_world,
    /// does not recreate a map and does not reset any temporal frame history.
    /// Failure is non-destructive, allowing the caller's compatibility fallback.
    pub(in crate::renderer) fn remap_resident_world(
        &mut self,
        map: &crate::scene::PreparedRemapPlan,
        base_texture_count: usize,
    ) -> Result<WorldUploadTimings, String> {
        let started = std::time::Instant::now();
        let Some(world) = self.world.as_ref() else { return Err("no resident world".into()) };
        if world.remap_asset_source != map.base.source || world.remap_asset_hash != map.base.map_hash
            || base_texture_count != world.remap_base_texture_count
            || map.base.textures.len() != base_texture_count
            || world.textures.len() < base_texture_count
        {
            return Err("resident map identity or original texture table differs".into());
        }
        // Cinematic/videoMap stages need a video player and map-level clock
        // registration; binding their initial frame alone would be incorrect.
        if !map.replacement_videos.is_empty() {
            return Err("videoMap replacement requires video registration".into());
        }
        let geometry = resident_geometry(world, map)?;
        // Validate every inline model's geometry ownership before staging the
        // update. The source flat stage counts are allowed to differ arbitrarily.
        if world.inline_models.len() != map.inline_models.len() {
            return Err("inline render instance count changed".into());
        }
        for model in &map.inline_models {
            if model.batches.end > map.inline_batches.len() {
                return Err("invalid inline remap stage range".into());
            }
        }
        let old_tex_prefix = &world.remap_texture_hashes[..base_texture_count.min(world.remap_texture_hashes.len())];
        let next_prefix = &map.base.texture_hashes[..base_texture_count.min(map.base.texture_hashes.len())];
        if old_tex_prefix != next_prefix {
            return Err("resident source texture content changed".into());
        }
        // Original texture slots never move. Replacement images are a suffix,
        // rebuilt once per remap; they never cause the source BSP to reupload.
        let mut textures = world.textures[..base_texture_count].to_vec();
        let mut texture_clamp = world.texture_clamp[..base_texture_count].to_vec();
        for (offset, texture) in map.replacement_textures.iter().enumerate() {
            let hash = map.replacement_texture_hashes.get(offset).copied();
            // Preserve image descriptors as well as hashes, so reordering the
            // suffix after a redirect change does not cause redundant uploads.
            // Changes to r_picmip are handled by the regular map reload path.
            let existing = hash.and_then(|hash| world.remap_texture_hashes.iter()
                .enumerate().skip(base_texture_count)
                .find(|(i, old)| **old == hash
                    && world.texture_clamp.get(*i).copied() == Some(texture.clamp))
                .map(|(i, _)| i));
            let image = if let Some(old) = existing {
                world.textures[old].clone()
            } else {
                let picmip = if texture.label.to_ascii_lowercase().starts_with("textures/") {
                    self.picmip
                } else { 0 };
                crate::renderer::upload_texture_picmip(
                    &self.device, &self.queue, texture, picmip,
                )
            };
            textures.push(image);
            texture_clamp.push(texture.clamp);
        }
        let primary_skybox = map.batches.iter().chain(map.inline_batches.iter())
            .find_map(|batch| batch.skybox);
        // Stage remaps must not accidentally retain a promoted ocean clipmap
        // when the new material wants a previously unavailable ocean mesh.
        // That rare geometry-changing transition uses the guarded fallback.
        for (sources, inline) in [(&map.batches, false), (&map.pvs_batches, false), (&map.inline_batches, true)] {
            for source in sources {
                if source.water && geometry[&geometry_key(source, inline)].iter().all(|b| !b.source.water) {
                    return Err("material remap introduces new ocean geometry".into());
                }
            }
        }
        let mut material_arena = UniformArena::new(&self.device, "JKA remapped material uniforms",
            std::mem::size_of::<crate::renderer::MaterialUniform>() as u64);
        let mut fog_arena = UniformArena::new(&self.device, "JKA remapped surface fog uniforms",
            std::mem::size_of::<weather::fog::SurfaceFogUniform>() as u64);
        let mut records = Vec::<GpuCullRecord>::new();
        let mut indirect = Vec::<DrawIndexedIndirectArgs>::new();
        // Command-local binding interning. A replacement applied to hundreds
        // of BSP pieces often needs only a handful of distinct materials
        // (lightmap/fog/probe combinations), never one bind group per triangle
        // range. This cache does not exist in frame rendering.
        let mut new_material_bindings = Vec::<WorldBatch>::new();
        let mut make_batches = |sources: &[DrawBatch], inline: bool| -> Vec<WorldBatch> {
            sources.iter().map(|source| {
                let candidates = &geometry[&geometry_key(source, inline)];
                let detail_word = source.texture.and_then(|index| world.detail_texture_by_base.get(index).copied()).unwrap_or(0);
                let uniform = material_uniform(source, (detail_word & AUTO_DETAIL_ENABLED_BIT) != 0);
                // Identical authored stages alias their existing GPU resources.
                // The same geometry can have many stages; matching the first
                // geometry owner unconditionally would accidentally restore
                // an earlier stage's texture/material after an N-to-M remap.
                let reusable = candidates.iter().copied().find(|old| {
                    let old_detail = old.source.texture.and_then(|i| world.detail_texture_by_base.get(i).copied()).unwrap_or(0);
                    let old_uniform = material_uniform(&old.source, (old_detail & AUTO_DETAIL_ENABLED_BIT) != 0);
                    crate::renderer::same_draw_state(&old.source, source)
                        // Suffix slots are recreated for each remap, so old
                        // bind groups may only be aliased for original slots.
                        && old.source.texture.is_none_or(|index| index < base_texture_count)
                        && old.source.normal_texture.is_none_or(|index| index < base_texture_count)
                        && old.source.roughness_texture.is_none_or(|index| index < base_texture_count)
                        && old.source.height_texture.is_none_or(|index| index < base_texture_count)
                        && old.source.metallic_texture.is_none_or(|index| index < base_texture_count)
                        && old.source.specular_texture.is_none_or(|index| index < base_texture_count)
                        && old.source.emissive_texture.is_none_or(|index| index < base_texture_count)
                        && old.source.skybox.into_iter().flatten().all(|index| index < base_texture_count)
                        && primary_skybox == world.primary_skybox
                        && bytemuck::bytes_of(&old_uniform) == bytemuck::bytes_of(&uniform)
                        && old.source.bsp_shader_index == source.bsp_shader_index
                        && old.source.material_debug_index == source.material_debug_index
                        && old.source.authored_glow == source.authored_glow
                        && old.source.water == source.water
                        && old.source.additive_companions.len() == source.additive_companions.len()
                        && old.source.additive_companions.iter().zip(&source.additive_companions)
                            .all(|(a,b)| a.texture.is_none_or(|index| index < base_texture_count)
                                && crate::renderer::same_draw_state(a,b)
                                && bytemuck::bytes_of(&material_uniform(a,false)) == bytemuck::bytes_of(&material_uniform(b,false)))
                });
                let shared = if reusable.is_none() {
                    new_material_bindings.iter().find(|prior| {
                        !source.water && !source.planar_reflection
                            && !source.planar_environment_candidate
                            && crate::renderer::same_draw_state(&prior.source, source)
                            && prior.source.reflection_probe == source.reflection_probe
                            && prior.source.detail_texture_eligible == source.detail_texture_eligible
                            && prior.source.surface_material == source.surface_material
                            && prior.source.authored_glow == source.authored_glow
                            && prior.source.additive_companions.len() == source.additive_companions.len()
                            && prior.source.additive_companions.iter().zip(&source.additive_companions)
                                .all(|(a,b)| crate::renderer::same_draw_state(a,b)
                                    && bytemuck::bytes_of(&material_uniform(a,false))
                                        == bytemuck::bytes_of(&material_uniform(b,false)))
                            && bytemuck::bytes_of(&material_uniform(
                                &prior.source,
                                prior.source.texture.and_then(|i| world.detail_texture_by_base.get(i).copied())
                                    .is_some_and(|word| (word & AUTO_DETAIL_ENABLED_BIT) != 0),
                            )) == bytemuck::bytes_of(&uniform)
                    })
                } else { None };
                // Physical index range, model, LOD and bounds must come from
                // THIS geometry, never from a material-binding cache neighbor.
                let template = reusable.unwrap_or(candidates[0]);
                let mut batch = (*template).clone();
                if let Some(prior) = shared { replace_stage_material(&mut batch, prior); }
                if reusable.is_none() && shared.is_none() {
                let fragment_key = MaterialFragmentKey::from_uniform(&uniform);
                let material_buffer = material_arena.push(&self.device, &self.queue, bytemuck::bytes_of(&uniform));
                let additive_uniforms: [UniformSlot; 2] = std::array::from_fn(|index| {
                    source.additive_companions.get(index).map_or_else(
                        || material_buffer.clone(),
                        |companion| {
                            let mut extra = material_uniform(companion, false);
                            extra.params[3] = source.shader_time_offset;
                            extra.vector_t[3] = source.shader_clamp_time.unwrap_or(0.0);
                            if companion.texture.and_then(|i| texture_clamp.get(i)).copied().unwrap_or(false) {
                                extra.header[3] |= 65536;
                            }
                            material_arena.push(&self.device, &self.queue, bytemuck::bytes_of(&extra))
                        },
                    )
                });
                let fog = weather::fog::surface_fog_uniform(source.fog, source.fog_is_global,
                    source.fog_color_override, source.legacy2_fog_in_stage_safe, source.global_fog_post_eligible);
                let fog_buffer = fog_arena.push(&self.device, &self.queue, bytemuck::bytes_of(&fog));
                let glow_fog_buffer = source.authored_glow.then(|| {
                    let glow = weather::fog::surface_fog_uniform(source.fog, source.fog_is_global,
                        weather::fog::FogColorOverride::Black,
                        source.legacy2_fog_in_stage_safe, source.global_fog_post_eligible);
                    fog_arena.push(&self.device, &self.queue, bytemuck::bytes_of(&glow))
                });
                let marks = std::array::from_fn(|slot| world.footprint_mark_textures[slot]
                    .and_then(|i| textures.get(i)).unwrap_or(&self.white));
                let detail_texture = crate::renderer::detail_texture_for_source(
                    source, &self.detail_texture, self.detail_texture_auto,
                    &self.detail_auto_textures, &world.detail_texture_by_base,
                );
                let full_group = |stage: &DrawBatch, fog_buffer: &UniformSlot| {
                    create_world_bind_group(
                        &self.device, &self.surface_layout, &self.white, &self.missing,
                        &self.flat_normal, &self.repeat_sampler, &self.clamp_sampler,
                        &self.pbr_repeat_sampler, &self.pbr_clamp_sampler, &self.lightmap_sampler,
                        &self.sky_sampler, detail_texture, &textures, &world.lightmaps,
                        &world.deluxemaps, &texture_clamp, primary_skybox, &marks,
                        &world.reflection_probes, stage, &material_buffer, &additive_uniforms,
                        fog_buffer, self.surface_deformation.buffer(),
                        self.surface_deformation.field_views(), self.surface_deformation.field_sampler(),
                    )
                };
                let fast_group = |stage: &DrawBatch| {
                    create_fast_world_bind_group(
                        &self.device, &self.fast_surface_layout, &self.white, &self.missing,
                        &self.repeat_sampler, &self.clamp_sampler, &self.lightmap_sampler,
                        &self.sky_sampler, &textures, &world.lightmaps, &texture_clamp,
                        primary_skybox, &marks, stage, &material_buffer, &additive_uniforms,
                        self.surface_deformation.buffer(), self.surface_deformation.field_views(),
                        self.surface_deformation.field_sampler(),
                    )
                };
                let bind_group = full_group(source, &fog_buffer);
                let fast_bind_group = fast_group(source);
                let authored_glow_bind_group = glow_fog_buffer.as_ref().map(|glow| full_group(source, glow));
                let animation = build_world_animation(source, &bind_group, &fast_bind_group,
                    &authored_glow_bind_group, |stage| WorldAnimationFrame {
                        full: full_group(stage, &fog_buffer),
                        fast: fast_group(stage),
                        glow: glow_fog_buffer.as_ref().map(|glow| full_group(stage, glow)),
                    });
                batch.fragment_key = fragment_key;
                batch.bind_group = bind_group;
                batch.fast_bind_group = fast_bind_group;
                batch.authored_glow_bind_group = authored_glow_bind_group;
                batch.animation = animation;
                batch._material_buffer = material_buffer;
                batch.additive_uniforms = additive_uniforms;
                batch._fog_buffer = fog_buffer;
                batch._authored_glow_fog_buffer = glow_fog_buffer;
                batch.source = source.clone();
                new_material_bindings.push(batch.clone());
                }
                batch.source = source.clone();
                if inline {
                    // Keep uploaded inline vertex namespace as on a full map
                    // upload, independent of shader-stage count.
                    let shift = map.base.vertices.len() as u32;
                    batch.source.vertices = batch.source.vertices.start.saturating_add(shift)
                        ..batch.source.vertices.end.saturating_add(shift);
                }
                // An authored ocean can continue to use its existing clipmap,
                // but replacing ocean shader with a normal material disables
                // the special ocean pass (never an overlay over the remap).
                if !source.water {
                    batch.ocean_clipmap = None;
                    batch.ocean_clipmap_id = u8::MAX;
                    batch.ocean_replaced = false;
                    batch.ocean_surface_bind_group = None;
                    batch._ocean_material_buffer = None;
                }
                batch.compact_group = None;
                batch.inline_group = u32::MAX;
                // Dynamic patch/model LOD can shrink indexed_range every frame;
                // reset to the resident full-detail range on a material change.
                batch.indexed_range = template.full_indexed_range.clone();
                batch.full_indexed_range = template.full_indexed_range.clone();
                batch.patch_geometry = template.patch_geometry;
                batch.patch_suppressed = template.patch_suppressed;
                let mut record = GpuCullRecord {
                    minimum: [batch.bounds_min[0], batch.bounds_min[1], batch.bounds_min[2], 0.0],
                    maximum: [batch.bounds_max[0], batch.bounds_max[1], batch.bounds_max[2], 0.0],
                    draw: [batch.indexed_range.end.saturating_sub(batch.indexed_range.start),
                        batch.indexed_range.start, u32::from(source.pipeline.class == DrawClass::Sky),
                        u32::from(inline)],
                    compact: [u32::MAX, 0, 0, 0],
                    face_plane: batch.face_cull_plane,
                };
                if source.pipeline.class == DrawClass::Transparent { record.draw[3] |= 8; }
                if source.water { record.draw[3] |= 1 << 8; }
                if !inline && map.base.material_debug.entries.get(source.material_debug_index as usize)
                    .is_some_and(|entry| entry.name.starts_with("models/")) { record.draw[3] |= 4; }
                record.draw[3] |= crate::renderer::world_blend_probe_bucket(source.pipeline) << 5;
                batch.cull_index = records.len() as u32;
                indirect.push(DrawIndexedIndirectArgs { index_count: record.draw[0],
                    instance_count: 1, first_index: record.draw[1], base_vertex: 0, first_instance: 0 });
                records.push(record);
                batch
            }).collect()
        };
        let mut coarse = make_batches(&map.batches, false);
        let mut full = if map.base.visibility.is_some() { make_batches(&map.pvs_batches, false) } else { Vec::new() };
        let inline_coarse_start = coarse.len();
        let inline_full_start = full.len();
        coarse.extend(make_batches(&map.inline_batches, true));
        if map.base.visibility.is_some() { full.extend(make_batches(&map.inline_batches, true)); }
        drop(make_batches);
        // A same-shape remap can patch just the WGPU material bindings: there
        // is no reason to allocate, clear or recreate GPU cull/indirect buffers,
        // AO state, AUTO4 collapse recipes or per-cluster visibility tables.
        // Build GPU materials only for changed authored stages (identical ones
        // are direct aliases), then validate that old draw-index topology still
        // represents the new stage configuration exactly.
        let compatible = compatible_live_stage_layout(world, map, &coarse, &full);
        if compatible {
            let old_auto4_variant_base = world.auto4_variant_base;
            drop(geometry);
            let world = self.world.as_mut().expect("resident world validated");
            let changed = world.coarse_batches.iter().zip(&coarse)
                .chain(world.full_batches.iter().zip(&full))
                .filter(|(a,b)| !crate::renderer::same_draw_state(&a.source, &b.source))
                .count();
            for (dst, src) in world.coarse_batches.iter_mut().zip(&coarse) {
                replace_stage_material(dst, src);
            }
            for (dst, src) in world.full_batches.iter_mut().zip(&full) {
                replace_stage_material(dst, src);
            }
            for (i, batch) in world.auto4_batches.iter_mut().enumerate() {
                let reference = if i < old_auto4_variant_base { Some(i) } else {
                    map.portal_draw_plan.variants.get(i - old_auto4_variant_base)
                        .map(|variant| variant.representative)
                };
                if let Some(src) = reference.and_then(|index| full.get(index)) {
                    if i < old_auto4_variant_base {
                        replace_stage_material(batch, src);
                    } else {
                        // AUTO4 recipes aggregate multiple FULL pieces. Their
                        // PVS/areaportal metadata is the union of those pieces,
                        // not the new representative's single-piece metadata.
                        let signature = batch.source.pvs_signature.clone();
                        let areas = batch.source.area_signature;
                        replace_stage_material(batch, src);
                        batch.source.pvs_signature = signature;
                        batch.source.area_signature = areas;
                    }
                }
            }
            world.textures = textures;
            world.texture_clamp = texture_clamp;
            world.remap_texture_hashes = map.base.texture_hashes.iter().copied()
                .chain(map.replacement_texture_hashes.iter().copied()).collect();
            world.animated_batches = None;
            let needs_pipelines = world.pipeline_variants.values().any(|variant|
                world.coarse_batches.iter().any(|batch| !variant.pipelines.contains_key(&batch.source.pipeline)));
            if needs_pipelines { self.rebuild_world_pipelines(); }
            rverbose!(1, "[SHADER REMAP] incremental GPU binding update {:.2} ms, {} changed-stage state(s); cull/PVS/AUTO4 buffers unchanged",
                started.elapsed().as_secs_f64() * 1000.0, changed);
            return Ok(WorldUploadTimings::default());
        }
        // Inline model slots are stable across shader pass-count changes, but
        // the per-instance contiguous range must be rewritten from the new
        // flattened stage table before the next snapshot arrives.
        let mut model_ranges = Vec::with_capacity(map.inline_models.len());
        for (slot, model) in map.inline_models.iter().enumerate() {
            model_ranges.push((
                (inline_coarse_start + model.batches.start .. inline_coarse_start + model.batches.end).collect::<Vec<_>>(),
                if map.base.visibility.is_some() {
                    (inline_full_start + model.batches.start .. inline_full_start + model.batches.end).collect::<Vec<_>>()
                } else { Vec::new() },
            ));
            for index in model.batches.clone() {
                coarse[inline_coarse_start + index].inline_model_slot = slot as u32;
                if map.base.visibility.is_some() { full[inline_full_start + index].inline_model_slot = slot as u32; }
            }
        }
        let mut inline_states = Vec::<DrawBatch>::new();
        for batch in coarse[inline_coarse_start..].iter_mut().chain(full[inline_full_start..].iter_mut()) {
            let group = inline_states.iter().position(|state| crate::renderer::same_draw_state(state, &batch.source))
                .unwrap_or_else(|| { inline_states.push(batch.source.clone()); inline_states.len() - 1 });
            batch.inline_group = group as u32;
        }
        let compact_groups = build_draw_compaction_groups(&mut records, &mut coarse, &mut full);
        let mut auto4 = if map.base.visibility.is_some() {
            instantiate_portal_draw_plans(&map.portal_draw_plan, &full, inline_full_start, &mut records, &mut indirect)
        } else { None };
        let mut auto4_states = Vec::new();
        if let Some(instance) = &auto4 {
            auto4_states = instance.geometry_states.iter().map(|state| match state {
                Auto4GeometryState::Static => Auto4GeometryState::Static,
                _ => Auto4GeometryState::Uncached,
            }).collect();
        }
        // Allocate all new cull and compaction descriptors on the command path;
        // ordinary frame rendering uses unchanged direct indexed lookups.
        if records.is_empty() {
            records.push(GpuCullRecord::zeroed());
            indirect.push(DrawIndexedIndirectArgs::zeroed());
        }
        let compact_capacity: u32 = compact_groups.iter().map(|g| g.max_count).sum();
        let cull_records_buffer = update_or_grow_buffer(
            &self.device, &self.queue, &world.cull_records_buffer,
            "JKA live-remap cull records", bytemuck::cast_slice(&records),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let indirect_buffer = update_or_grow_buffer(
            &self.device, &self.queue, &world.indirect_buffer,
            "JKA live-remap indirect args", bytemuck::cast_slice(&indirect),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        );
        // The Hi-Z prediction buffer lacks COPY_DST and therefore cannot be
        // reset by Queue::write_buffer. Recreate this small scratch allocation.
        let early_indirect_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA live-remap Hi-Z early indirect"),
            size: (records.len() * std::mem::size_of::<DrawIndexedIndirectArgs>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
            mapped_at_creation: false,
        });
        let active_cull_indices_buffer = update_or_grow_buffer(
            &self.device, &self.queue, &world.active_cull_indices_buffer,
            "JKA live-remap active indices", bytemuck::cast_slice(&vec![0u32; records.len()]),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let compact_indirect_buffer = if world.compact_indirect_buffer.size()
            >= (compact_capacity.max(1) as u64) * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64 {
            world.compact_indirect_buffer.clone()
        } else {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA live-remap compact args"),
                size: (compact_capacity.max(1) as u64) * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let reflection_compact_capacity = compact_capacity.saturating_mul(PLANAR_REFLECTION_SLOTS as u32).max(1);
        let reflection_compact_indirect_buffer = if world.reflection_compact_indirect_buffer.size()
            >= reflection_compact_capacity as u64 * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64 {
            world.reflection_compact_indirect_buffer.clone()
        } else {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA live-remap reflection compact args"),
                size: reflection_compact_capacity as u64 * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        // Keep mover indirect memory resident when a material loses passes.
        let inline_indirect_capacity = map.inline_batches.len().max(1) as u32;
        let inline_bytes = (inline_indirect_capacity.max(1) as u64)
            * (PLANAR_REFLECTION_SLOTS as u64 + 1)
            * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64;
        let inline_indirect_buffer = if world.inline_indirect_buffer.size() >= inline_bytes {
            world.inline_indirect_buffer.clone()
        } else {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA live-remap mover multi-draw"),
                size: inline_bytes,
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let compact_count_buffer = update_or_grow_buffer(
            &self.device, &self.queue, &world.compact_count_buffer,
            "JKA live-remap compact counts", bytemuck::cast_slice(&vec![0u32; compact_groups.len().max(1)]),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        );
        let cull_debug_reason_buffer = update_or_grow_buffer(
            &self.device, &self.queue, &world.cull_debug_reason_buffer,
            "JKA live-remap cull reasons", bytemuck::cast_slice(&vec![0u32; records.len()]),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let cull_debug_count_buffer = update_or_grow_buffer(
            &self.device, &self.queue, &world.cull_debug_count_buffer,
            "JKA live-remap cull counts", bytemuck::cast_slice(&[0u32; 4]),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        let cull_debug_bind_group = create_cull_debug_bind_group(
            &self.device, &self.cull_debug_layout, &cull_debug_reason_buffer);
        let cull_bind_group = create_gpu_cull_bind_group(
            &self.device, &self.gpu_cull_layout, &cull_records_buffer, &indirect_buffer,
            &self.targets.hiz_view, &self.gpu_cull_settings_buffer, &active_cull_indices_buffer,
            &compact_indirect_buffer, &compact_count_buffer, &cull_debug_reason_buffer,
            &cull_debug_count_buffer, &early_indirect_buffer,
        );
        let old_cache_indices = world.auto4_collapse_cache.capacity_indices;
        let source_indices = Arc::clone(&world.remap_index_source);
        let old_primary_skybox = world.primary_skybox;
        // Phase 2: all validation, GPU material creation and buffer allocation
        // succeeded. Commit atomically to the existing WorldGpu.
        let world = self.world.as_mut().expect("world validated above");
        world.textures = textures;
        world.texture_clamp = texture_clamp;
        world.remap_texture_hashes = map.base.texture_hashes.iter().copied().chain(map.replacement_texture_hashes.iter().copied()).collect();
        world.primary_skybox = primary_skybox;
        let sky_shaders = map.batches.iter().filter(|b| b.pipeline.class == DrawClass::Sky)
            .map(|b| b.bsp_shader_index).collect::<BTreeSet<_>>();
        let boxed_shaders = map.batches.iter().filter(|b| b.pipeline.class == DrawClass::Sky && b.skybox.is_some())
            .map(|b| b.bsp_shader_index).collect::<BTreeSet<_>>();
        world.sky_covers_background = sky_shaders == boxed_shaders;
        world.coarse_batches = coarse;
        world.full_batches = full;
        world.cull_records_buffer = cull_records_buffer;
        world.indirect_buffer = indirect_buffer;
        world.early_indirect_buffer = early_indirect_buffer;
        world.active_cull_indices_buffer = active_cull_indices_buffer;
        world.compact_indirect_buffer = compact_indirect_buffer;
        world.reflection_compact_indirect_buffer = reflection_compact_indirect_buffer;
        world.reflection_compact_capacity = reflection_compact_capacity;
        world.inline_indirect_buffer = inline_indirect_buffer;
        world.inline_indirect_capacity = inline_indirect_capacity.max(1);
        world.inline_group_count = inline_states.len();
        world.compact_count_buffer = compact_count_buffer;
        world.cull_debug_reason_buffer = cull_debug_reason_buffer;
        world.cull_debug_count_buffer = cull_debug_count_buffer;
        world.cull_debug_bind_group = cull_debug_bind_group;
        world.cull_bind_group = Some(cull_bind_group);
        world.cull_record_count = records.len();
        world.compact_groups = compact_groups;
        world.animated_batches = None;
        world.active_selection_key = None;
        world.last_cluster = None;
        world.ocean_gate_cache = None;
        world.coarse_all_batches = (0..world.coarse_batches.len()).collect();
        world.full_all_batches = if world.sky_portal.is_some() { (0..world.full_batches.len()).collect() } else { Vec::new() };
        world.coarse_visible_batches_by_cluster = world.visibility.as_ref().map(|vis|
            visible_batches_by_cluster(&world.coarse_batches, vis)).unwrap_or_default();
        world.full_visible_batches_by_cluster = world.visibility.as_ref().map(|vis|
            visible_batches_by_cluster(&world.full_batches, vis)).unwrap_or_default();
        for (model, (coarse_range, full_range)) in world.inline_models.iter_mut().zip(model_ranges) {
            model.coarse_batches = coarse_range;
            model.full_batches = full_range;
        }
        world.patch_lod.clear_batch_bindings();
        world.patch_lod.bind(&mut world.coarse_batches, 0);
        world.patch_lod.bind(&mut world.full_batches, 1);
        world.bsp_model_lod.bind(&world.coarse_batches, &world.full_batches);
        if let Some(instance) = &mut auto4 {
            world.patch_lod.bind(&mut instance.batches[..instance.variant_base], 2);
            world.patch_lod.recipes(&instance.geometry_members, &world.full_batches, &mut instance.geometry_states);
            world.bsp_model_lod.recipes(&instance.geometry_members, &world.full_batches, &mut instance.geometry_states);
            for (index, state) in instance.geometry_states.iter().enumerate() {
                if matches!(state, Auto4GeometryState::Uncached) {
                    for &variant in &instance.geometry_variants[index] {
                        instance.batches[instance.variant_base + variant].indexed_range = 0..0;
                    }
                }
            }
            auto4_states = instance.geometry_states.iter().map(|state| match state {
                Auto4GeometryState::Static => Auto4GeometryState::Static,
                _ => Auto4GeometryState::Uncached,
            }).collect();
        }
        if let Some(instance) = auto4 {
            world.auto4_batches = instance.batches;
            world.auto4_variant_base = instance.variant_base;
            world.auto4_always_start = instance.always_start;
            world.auto4_variant_members = instance.variant_members;
            world.auto4_variant_geometry = instance.variant_geometry;
            world.auto4_variant_area_mixed = instance.variant_area_mixed;
            world.auto4_geometry_members = instance.geometry_members;
            world.auto4_geometry_variants = instance.geometry_variants;
            world.auto4_inline_start = inline_full_start;
            world.auto4_inline_full_start = inline_full_start;
        } else {
            world.auto4_batches.clear();
            world.auto4_variant_base = 0;
            world.auto4_always_start = 0;
            world.auto4_variant_members.clear();
            world.auto4_variant_geometry.clear();
            world.auto4_variant_area_mixed.clear();
            world.auto4_geometry_members.clear();
            world.auto4_geometry_variants.clear();
            world.auto4_inline_start = 0;
            world.auto4_inline_full_start = 0;
        }
        // AUTO4's newly instantiated recipe cull records must carry exactly
        // the same material-class bits as normal world stages. The initial
        // per-stage cull records were tagged above; recipe records are created
        // separately by instantiate_portal_draw_plans.
        for batch in world.auto4_batches.iter() {
            if let Some(record) = records.get_mut(batch.cull_index as usize) {
                if map.base.material_debug.entries.get(batch.source.material_debug_index as usize)
                    .is_some_and(|entry| entry.name.starts_with("models/")) { record.draw[3] |= 4; }
                if batch.source.pipeline.class == DrawClass::Transparent { record.draw[3] |= 8; }
                record.draw[3] |= crate::renderer::world_blend_probe_bucket(batch.source.pipeline) << 5;
                record.draw[3] |= u32::from(batch.source.water) << 8;
            }
        }
        // Patch / compiled-model LOD may have replaced indexed ranges after
        // the batch table was rebuilt. Publish exactly those final ranges to
        // the GPU culler as one contiguous upload per buffer (not per draw).
        for batch in world.coarse_batches.iter().chain(world.full_batches.iter()).chain(world.auto4_batches.iter()) {
            if let (Some(record), Some(args)) = (records.get_mut(batch.cull_index as usize),
                indirect.get_mut(batch.cull_index as usize)) {
                record.draw[0] = batch.indexed_range.end.saturating_sub(batch.indexed_range.start);
                record.draw[1] = batch.indexed_range.start;
                record.face_plane = batch.face_cull_plane;
                args.index_count = record.draw[0];
                args.first_index = record.draw[1];
            }
        }
        self.queue.write_buffer(&world.cull_records_buffer, 0, bytemuck::cast_slice(&records));
        self.queue.write_buffer(&world.indirect_buffer, 0, bytemuck::cast_slice(&indirect));
        world.auto4_plan_refs = map.portal_draw_plan.plans.clone();
        world.auto4_plan_by_cluster = map.portal_draw_plan.plan_by_cluster.clone();
        world.auto4_active_plan.clear();
        world.auto4_collapse_cache = Auto4CollapseCache::new(auto4_states);
        world.auto4_collapse_cache.capacity_indices = old_cache_indices;
        let base_end = source_indices.len() as u32;
        if old_cache_indices > 0 && !world.auto4_collapse_cache.states.is_empty() {
            world.auto4_collapse_cache.free_ranges.push(base_end..base_end + old_cache_indices);
            let piece_ranges = Arc::new(world.full_batches.iter().take(inline_full_start)
                .map(|batch| batch.full_indexed_range.clone()).collect::<Vec<Range<u32>>>());
            match Auto4CollapseWorker::spawn(source_indices, piece_ranges) {
                Ok(worker) => world.auto4_collapse_cache.worker = Some(worker),
                Err(e) => rverbose!(1, "[SHADER REMAP] AUTO4 cold collapse unavailable: {e}"),
            }
        }
        world.has_glass_primary = world.coarse_batches.iter().any(|b| b.source.glass_primary);
        world.material_debug = map.base.material_debug.clone();
        // Batch keys changed in place: renumber slots before any frame uses them.
        world.refresh_pso_slots();
        // Shader stage changes may introduce new PSO keys. Rebuild PSOs only
        // on the cold command; unchanged formats/geometry never go through it.
        let needs_pipelines = world.pipeline_variants.values().any(|variant|
            world.coarse_batches.iter().any(|batch| !variant.pipelines.contains_key(&batch.source.pipeline)));
        if needs_pipelines { self.rebuild_world_pipelines(); }
        if old_primary_skybox != primary_skybox {
            rverbose!(1, "[SHADER REMAP] primary skybox changed; existing world GPU geometry preserved");
        }
        self.map_has_water = self.world.as_ref().is_some_and(|w| w.coarse_batches.iter().any(|b| b.source.water));
        self.map_has_snow_surfaces = self.world.as_ref().is_some_and(|w| w.coarse_batches.iter().any(|b|
            u32::from(b.source.surface_material) == crate::renderer::MATERIAL_SNOW && b.source.pipeline.class != DrawClass::Sky));
        rverbose!(1, "[SHADER REMAP] live stage replacement completed in {:.2} ms; BSP vertex/index buffers, PVS, RT source and map state retained", started.elapsed().as_secs_f64() * 1000.0);
        Ok(WorldUploadTimings::default())
    }
}
