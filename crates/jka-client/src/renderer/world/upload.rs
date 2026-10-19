//! World upload.
use crate::renderer::{
    average_skybox_color, build_draw_compaction_groups, collect_planar_reflectors,
    collect_static_ao_lightmap_triangles, collect_static_ao_render_triangles,
    collect_static_ao_vertex_receivers, create_cull_debug_bind_group, create_fast_world_bind_group,
    build_world_animation, WorldAnimationFrame,
    create_legacy_dlight_pass_pipelines, create_legacy_fog_pass_pipelines,
    create_local_shadow_resources, create_reflection_fog_pass_pipelines,
    create_reflection_pipelines, create_snow_shell_gpu, create_world_bind_group,
    create_specialized_world_pipelines, create_world_pipelines, detail_texture_for_source,
    material_specialization_eligible, material_specialization_jobs, world_base_fragment_entry,
    MaterialFragmentKey, gpu_point_lights, inspector_texture_meta,
    instantiate_portal_draw_plans, is_upward_water_face, legacy_dlight_runs_for_batch, packed_face_plane, authored_face_runs,
    make_cull_record, material_uniform, ocean_surface_key, rt_alpha_mask_source_from_gpu_vertices, rt_glass_source_from_gpu_vertices,
    same_draw_state, scene, world_blend_probe_bucket, upload_reflection_probes, upload_static_light_grid, upload_texture,
    upload_texture_picmip, upload_voxel_probe_gi,
    visible_batches_by_cluster, water_surface_extent, weather, Arc, Auto4CollapseCache,
    Auto4CollapseWorker, Auto4GeometryState, Auto4Instance, BTreeMap, BTreeSet, BlendMode,
    DrawBatch, DrawClass, DrawIndexedIndirectArgs, GpuCullRecord, GpuImage, GpuPointLight,
    GpuVertex, GrassRenderer, HashMap, IndexedGeometryBuilder, InlineModelGpu, InspectorVertex,
    Instant, MaterialUniform, PreparedMap, PreparedPortalPlanBatchRef, RtAlphaMaskSource,
    RuntimeOceanClipmap, OceanPromotionDecision,
    StaticAoBaseLightmap, StaticAoWorldSource, SurfaceSpriteEffectGpu, SurfaceSpriteEffectRenderer,
    texture_compression, TextureData, UniformArena, UniformSlot, WorldBatch, WorldGpu, WorldPipelineVariant, WorldShaderVariantKey,
    WorldUploadTimings, AUTO4_COLLAPSE_CACHE_BYTES, AUTO_DETAIL_ENABLED_BIT, CLUSTER_COUNT,
    LOCAL_SHADOW_CACHE_SLOTS, MAX_CLUSTER_LIGHTS, MAX_DYNAMIC_LIGHTS, MAX_LOCAL_SHADOW_LIGHTS,
    MAX_OCEAN_SURFACES, PLANAR_REFLECTION_SLOTS,
};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

/// r_colorClearMode 1 may skip the world colour clear only if every sky shader on
/// the map draws an outer box, or the map has no sky at all (enclosed): then any
/// pixel the world misses is a void, which vanilla never clears either (hall of
/// mirrors). A `skyParms -` sky discards and shows the clear colour, so one of
/// those keeps the clear.
/// JKA_DUMP_WORLD_BATCHES=<file>: one line per uploaded world batch with its index
/// range (to match GPU-capture draws), the base-pass fragment entry point, the
/// override constants its PSO is compiled with in each r_fragmentShaderMode, and
/// the material name. Diagnostics only.
fn dump_world_batches(
    path: &std::ffi::OsStr,
    coarse: &[WorldBatch],
    full: &[WorldBatch],
    variant: WorldShaderVariantKey,
    legacy_fog: bool,
    material_debug: &scene::MaterialDebugInfo,
) {
    use std::fmt::Write as _;
    let base_variant = variant.base_pass_key();
    let mut out = String::new();
    let _ = writeln!(out, "variant\t{}\t{}", base_variant.short_label(),
        base_variant.compilation_constants().iter()
            .map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join(" "));
    for (set, batches) in [("coarse", coarse), ("full", full)] {
        for batch in batches {
            let key = batch.source.pipeline;
            let entry = world_base_fragment_entry(key, legacy_fog, true);
            let alpha_test = key.class != DrawClass::Opaque;
            let material = batch.fragment_key.for_variant(variant, alpha_test);
            let specialized = material_specialization_eligible(&key);
            let name = material_debug.entries.get(batch.source.material_debug_index as usize)
                .map_or("?", |entry| entry.name.as_str());
            let _ = writeln!(out, "{set}\t{}\t{}\t{entry}\t{:?}\tENABLE_ALPHA_TEST={}\t{}\t{}\t{:?}",
                batch.indexed_range.start, batch.indexed_range.end - batch.indexed_range.start,
                key.class, u8::from(alpha_test),
                if specialized {
                    material.compilation_constants().iter()
                        .map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join(" ")
                } else {
                    "general".to_owned()
                },
                name, key.blend);
        }
    }
    match std::fs::write(path, out) {
        Ok(()) => println!("JKA_DUMP_WORLD_BATCHES: wrote {}", std::path::Path::new(path).display()),
        Err(error) => println!("JKA_DUMP_WORLD_BATCHES: {error}"),
    }
}

fn sky_outer_box_covers_background<'a>(sources: impl Iterator<Item = &'a DrawBatch>) -> bool {
    let mut sky_shaders = BTreeSet::new();
    let mut boxed_shaders = BTreeSet::new();
    for source in sources.filter(|source| source.pipeline.class == DrawClass::Sky) {
        sky_shaders.insert(source.bsp_shader_index);
        if source.skybox.is_some() {
            boxed_shaders.insert(source.bsp_shader_index);
        }
    }
    sky_shaders == boxed_shaders
}

pub(in crate::renderer) fn build_world(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface_layout: &wgpu::BindGroupLayout,
    fast_surface_layout: &wgpu::BindGroupLayout,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    shader_variant: WorldShaderVariantKey,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    ray_tracing_supported: bool,
    legacy_fog: bool,
    white: &GpuImage,
    missing: &GpuImage,
    flat_normal: &GpuImage,
    repeat_sampler: &wgpu::Sampler,
    clamp_sampler: &wgpu::Sampler,
    pbr_repeat_sampler: &wgpu::Sampler,
    pbr_clamp_sampler: &wgpu::Sampler,
    lightmap_sampler: &wgpu::Sampler,
    sky_sampler: &wgpu::Sampler,
    detail_texture: &GpuImage,
    detail_texture_auto: bool,
    detail_auto_textures: &HashMap<String, GpuImage>,
    detail_texture_by_base: Vec<u8>,
    cluster_compute_layout: &wgpu::BindGroupLayout,
    lighting_layout: &wgpu::BindGroupLayout,
    lighting_layout_lean: &wgpu::BindGroupLayout,
    shadow_caster_layout: &wgpu::BindGroupLayout,
    cull_debug_layout: &wgpu::BindGroupLayout,
    lighting_settings_buffer: &wgpu::Buffer,
    pbr_settings_buffer: &wgpu::Buffer,
    surface_deformation_buffer: &wgpu::Buffer,
    deformation_field_views: [&wgpu::TextureView; 2],
    deformation_field_sampler: &wgpu::Sampler,
    grass_renderer: &GrassRenderer,
    surface_sprite_effect_renderer: &SurfaceSpriteEffectRenderer,
    build_ocean_meshes: bool,
    compress_pbr_companions: bool,
    picmip: u32,
    texture_compression: u8,
    map: PreparedMap,
    reuse_world: Option<&WorldGpu>,
    requested_base_texture_count: usize,
) -> (WorldGpu, WorldUploadTimings) {
    // An R_RemapShader changes stage/material tables, not original decoded
    // images. Cache keys are checked here only on the remap command path;
    // ordinary map loads and frame rendering do no additional work.
    let remap_asset_source = map.source.clone();
    let remap_asset_hash = map.map_hash;
    let remap_texture_hashes = map.texture_hashes.clone();
    let remap_base_texture_count = if reuse_world.is_some() {
        requested_base_texture_count
    } else {
        map.textures.len()
    };
    let source_map = map.map_file_stats.is_some();
    let sky_portal = map.sky_portal;
    let PreparedMap {
        vertices,
        patch_draws,
        inline_patch_draws,
        legacy_dlight_triangle_surfaces,
        bsp_compiled_model_surfaces,
        legacy_dlight_surfaces,
        batches: coarse_sources,
        pvs_batches: full_sources,
        portal_draw_plan,
        textures: texture_data,
        footprint_mark_textures,
        lightmaps: lightmap_data,
        deluxemaps: deluxemap_data,
        static_bsp_ao_cache,
        visibility,
        lights: dynamic_lights,
        source_map_lighting,
        baked_sun_encoding,
        sun,
        static_light_grid,
        voxel_probe_gi: voxel_probe_gi_data,
        reflection_probes: reflection_probe_data,
        grass_patches,
        grass_local_fogs,
        surface_sprite_effects: surface_sprite_effect_sources,
        surface_sprite_bake,
        material_debug,
        inline_vertices,
        inline_batches: inline_batch_sources,
        inline_models: inline_model_sources,
        mark_surfaces,
        ..
    } = map;
    // Decide texture usages immediately after receiving the prepared map. BC
    // encoding depends on this immutable material classification, NOT on GPU
    // state, so workers can encode while the renderer builds indexed geometry.
    let texture_classify_started = Instant::now();
    // Companion maps are only safe to BC-compress when that image is not also
    // referenced as authored color/emissive/sky data. The optional benchmark
    // path uses BC3 because it preserves all RGB channels plus alpha (required
    // by normalHeightMap and rmosMap) without changing shader decoding.
    const TEX_USE_COMPANION: u8 = 1;
    const TEX_USE_COLOR: u8 = 2;
    const TEX_NO_PICMIP: u8 = 4;
    let mut texture_usage = vec![0u8; texture_data.len()];
    let mut mark = |index: Option<usize>, usage: u8| {
        if let Some(index) = index {
            if let Some(bits) = texture_usage.get_mut(index) {
                *bits |= usage;
            }
        }
    };
    // Ambient illumination is a property of the authored skybox, not the
    // number of geometry/PVS batches which happen to reference it. Select the
    // same primary skybox used for world rendering, without consulting FULL/
    // AUTO PVS pieces. Include inline sources for source-map edge cases.
    let primary_skybox = coarse_sources
        .iter()
        .chain(inline_batch_sources.iter())
        .find_map(|source| source.skybox);
    for source in coarse_sources.iter().chain(full_sources.iter()).chain(inline_batch_sources.iter()) {
        mark(source.texture, TEX_USE_COLOR);
        if source.no_picmip {
            mark(source.texture, TEX_NO_PICMIP);
            if let Some(animation) = &source.animation {
                for &index in &animation.frames { mark(Some(index), TEX_NO_PICMIP); }
            }
            if let Some(skybox) = source.skybox {
                for index in skybox { mark(Some(index), TEX_NO_PICMIP); }
            }
        }
        mark(source.emissive_texture, TEX_USE_COLOR);
        mark(source.normal_texture, TEX_USE_COMPANION);
        mark(source.roughness_texture, TEX_USE_COMPANION);
        mark(source.height_texture, TEX_USE_COMPANION);
        mark(source.metallic_texture, TEX_USE_COMPANION);
        mark(source.specular_texture, TEX_USE_COMPANION);
        if let Some(skybox) = source.skybox {
            for index in skybox {
                mark(Some(index), TEX_USE_COLOR);
            }
        }
    }
    for index in footprint_mark_textures.into_iter().flatten() {
        mark(Some(index), TEX_USE_COLOR);
    }
    for effect in &surface_sprite_effect_sources {
        mark(Some(effect.texture), TEX_USE_COLOR);
    }
    let texture_classify_ms = texture_classify_started.elapsed().as_secs_f64() * 1000.0;
    // Move the source vectors into Arc without copying texture bytes. Renderer
    // uploads and BC encoder workers only borrow the same immutable pixels.
    let texture_data = Arc::new(texture_data);
    let mut lightmap_data = lightmap_data;
    let authored_sun_bake = sun.zip(baked_sun_encoding).and_then(|(sun, encoding)| {
        crate::renderer::lighting::original_sun::bake_authored_sun_masks(
            &vertices, &coarse_sources, &texture_data, &mut lightmap_data,
            &legacy_dlight_triangle_surfaces, legacy_dlight_surfaces.len(), sun, encoding,
        )
    });
    let authored_sun_projected = authored_sun_bake.is_some();
    let lightmap_data = Arc::new(lightmap_data);
    let baked_sun_calibration = authored_sun_bake.or_else(|| sun.and_then(|sun| {
        crate::renderer::lighting::original_sun::estimate_baked_sun_radiance(
            &vertices, &coarse_sources, &lightmap_data,
            &legacy_dlight_triangle_surfaces, &legacy_dlight_surfaces, sun,
        )
    }));
    let baked_sun_radiance = baked_sun_calibration.as_ref().map(|fit| fit.radiance);
    // Existing GPU textures are safe to alias only when *all* decode/upload
    // inputs agree. In particular, the use classification can change BC
    // format, detail treatment, or picmip eligibility after a remap.
    let reusable_world = reuse_world.filter(|previous| {
        previous.remap_asset_source == remap_asset_source
            && previous.remap_asset_hash == remap_asset_hash
            && previous.remap_base_texture_count >= remap_base_texture_count
            && previous.remap_picmip == picmip
            && previous.remap_compression == texture_compression
            && previous.remap_pbr_compression == compress_pbr_companions
    });
    let mut old_by_hash = HashMap::<u64, usize>::new();
    if let Some(previous) = reusable_world {
        for (i, &hash) in previous.remap_texture_hashes.iter().enumerate() {
            if hash != 0 && i < previous.textures.len() {
                old_by_hash.entry(hash).or_insert(i);
            }
        }
    }
    let reusable_textures = texture_usage.iter().enumerate().map(|(index, &usage)| {
        let previous = reusable_world?;
        let hash = *remap_texture_hashes.get(index).unwrap_or(&0);
        // Unhashed procedural images can be reused only within the original
        // unchanged map-texture prefix, never by name or guesswork.
        let candidate = if index < remap_base_texture_count {
            Some(index)
        } else if hash != 0 {
            old_by_hash.get(&hash).copied()
        } else {
            None
        }?;
        (previous.textures.get(candidate).is_some()
            && previous.remap_texture_usage.get(candidate) == Some(&usage)
            && (hash == 0 && index < remap_base_texture_count
                || hash != 0 && previous.remap_texture_hashes.get(candidate) == Some(&hash)))
            .then_some(candidate)
    }).collect::<Vec<_>>();
    let reuse_bitmap = reusable_textures.iter().map(Option::is_some).collect::<Vec<_>>();
    let reused_count = reuse_bitmap.iter().filter(|&&v| v).count();
    if reused_count != 0 {
        rverbose!(1, "[SHADER REMAP] GPU texture reuse: {reused_count}/{} images", texture_usage.len());
    }
    // The original lightmap pages are immutable across remaps. Compare the
    // actual decoded data (not filenames) before aliasing GPU lightmaps, and
    // decline the shortcut while a runtime AO replacement is active.
    let reusable_lightmaps = reusable_world.filter(|previous| {
        !previous.static_ao_lightmap_active
            && !previous.baked_sun_cache.as_ref().is_some_and(|cache| cache.active)
            && previous.lightmaps.len() == lightmap_data.len()
            && previous.lightmap_base_data.len() == lightmap_data.len()
            && previous.lightmap_base_data.iter().zip(lightmap_data.iter()).all(|(a, b)| {
                a.width == b.width && a.height == b.height
                    && a.mip_level_count == b.mip_level_count
                    && a.srgb == b.srgb && a.clamp == b.clamp
                    && a.rgba == b.rgba && a.rgba16f == b.rgba16f
            })
    });
    if reusable_lightmaps.is_some() {
        rverbose!(1, "[SHADER REMAP] reused {} resident lightmaps", lightmap_data.len());
    }
    let color_encode_rx = if texture_compression >= 1 || compress_pbr_companions {
        let data = Arc::clone(&texture_data);
        let usage = texture_usage.clone();
        let skip = reuse_bitmap.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        rayon::spawn(move || {
            let started = Instant::now();
            let encoded = texture_compression::encode_world_textures_skipping(
                &data, &usage, picmip, texture_compression >= 1, compress_pbr_companions, &skip,
            );
            let _ = tx.send((encoded, started.elapsed().as_secs_f64() * 1000.0));
        });
        Some(rx)
    } else {
        None
    };
    // Inverse exposure needs the original bytes and sun-flux alpha. Lossy BC
    // quantization can turn a small byte error into a large photon error.
    let lightmap_encode_rx = if texture_compression >= 2 && !authored_sun_projected && reusable_lightmaps.is_none() {
        let data = Arc::clone(&lightmap_data);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        rayon::spawn(move || {
            let started = Instant::now();
            let encoded = texture_compression::encode_textures(&data, |_| Some(0));
            let _ = tx.send((encoded, started.elapsed().as_secs_f64() * 1000.0));
        });
        Some(rx)
    } else {
        None
    };
    let snow_started = Instant::now();
    let snow_shell = create_snow_shell_gpu(device, &vertices, &coarse_sources);
    let snow_shell_ms = snow_started.elapsed().as_secs_f64() * 1000.0;
    let prelude_started = Instant::now();
    let inspector_vertices = vertices
        .iter()
        .map(|vertex| InspectorVertex {
            position: vertex.position,
            normal: vertex.normal,
            uv: vertex.uv,
            lightmap_uv: vertex.lightmap_uv,
            color: vertex.color,
        })
        .collect::<Vec<_>>();
    let inspector_textures = texture_data
        .iter()
        .map(inspector_texture_meta)
        .collect::<Vec<_>>();
    let inspector_lightmaps = lightmap_data
        .iter()
        .map(inspector_texture_meta)
        .collect::<Vec<_>>();
    let static_ao_lightmap_triangles =
        collect_static_ao_lightmap_triangles(&vertices, &coarse_sources);
    let static_ao_vertex_receivers =
        collect_static_ao_vertex_receivers(vertices.len(), &coarse_sources);
    let static_ao_render_triangles = collect_static_ao_render_triangles(&vertices, &coarse_sources);
    let static_ao_lightmap_sizes = lightmap_data
        .iter()
        .map(|image| [image.width, image.height])
        .collect::<Vec<_>>();
    let static_ao_lightmap_bases = lightmap_data
        .iter()
        .map(|image| StaticAoBaseLightmap {
            label: image.label.clone(),
            width: image.width,
            height: image.height,
            rgba: image.rgba.clone(),
            clamp: image.clamp,
            srgb: image.srgb,
        })
        .collect::<Vec<_>>();
    let static_ao_cache_source = static_bsp_ao_cache;

    let mut upload_timings = WorldUploadTimings::default();
    upload_timings.snow_shell_ms = snow_shell_ms;
    upload_timings.texture_classify_ms = texture_classify_ms;
    upload_timings.prelude_ms = prelude_started.elapsed().as_secs_f64() * 1000.0;

    let vertex_started = Instant::now();
    let mut vertex_gpu_calls_ms = 0.0;
    let mut indexed_builder =
        IndexedGeometryBuilder::new(&vertices, &legacy_dlight_triangle_surfaces);
    let coarse_index_ranges = coarse_sources
        .iter()
        .map(|source| indexed_builder.batch_range(source))
        .collect::<Vec<_>>();
    let full_index_ranges = full_sources
        .iter()
        .map(|source| indexed_builder.batch_range(source))
        .collect::<Vec<_>>();
    // AUTO 4 recipes concatenate FULL piece index ranges. The map worker only
    // decided which pieces each recipe holds; non-contiguous recipes are built
    // lazily on `jka-auto4-collapse`, drawing their FULL pieces meanwhile.
    let auto4_piece_ranges = Arc::new(full_index_ranges.clone());
    // The FFT ocean replaces every promoted water face with one shared
    // camera-centred clipmap, so the authored brush footprint no longer has to
    // be subdivided per map. `build_ocean_meshes` only decides whether the
    // clipmap is worth uploading at all.
    // Contiguous water on one plane is one ocean surface, and the surface cap
    // goes to the biggest ones first. Each surface gets one clipmap that bakes in
    // its plane height and footprint, so the vertex shader needs nothing from the
    // material uniform and cannot end up placing the ocean somewhere the draw
    // call did not intend. The footprint is only the bounding box; the real
    // outline travels as triangle masks the fragment shader cuts back to.
    let mut ocean_clipmaps = BTreeMap::<[u32; 6], ([std::ops::Range<u32>; 2], u8)>::new();
    let mut ocean_surfaces = Vec::<crate::ocean::OceanSurface>::new();
    let mut ocean_promotion_decisions = BTreeMap::<[u32; 6], OceanPromotionDecision>::new();
    let mut ocean_appearances = BTreeMap::new();
    let mut ocean_masks = crate::ocean::OceanMasks::default();
    // Sky contacts are directions, NOT geometry; the finite clipmap follows
    // the camera at runtime. Do not feed this data to coastal/bathymetry.
    let mut ocean_sky_regions = vec![None; crate::ocean::OCEAN_MAX_SURFACES];
    let mut runtime_ocean_clipmaps = Vec::<RuntimeOceanClipmap>::new();
    let mut ocean_coarsest_spacing = 0.0f32;
    let ocean_mesh_started = Instant::now();
    let mut diffuse_stages = HashMap::new();
    if build_ocean_meshes {
        for source in coarse_sources.iter().chain(full_sources.iter()) {
            if source.water && !source.texture_is_lightmap
                && !matches!(source.tc_gen,jka_assets::shader::TcGen::Environment) {
                let stage = diffuse_stages.entry((source.vertices.start,source.vertices.end))
                    .or_insert_with(|| source.clone());
                if stage.texture.is_none() && source.texture.is_some() { *stage = source.clone(); }
            }
        }
    }

    if build_ocean_meshes {
        let mut water_faces = Vec::<crate::ocean::WaterFace>::new();
        let mut water_face_keys = Vec::<[u32; 6]>::new();
        let mut water_face_appearances = Vec::new();
        let mut water_face_sources = Vec::new();
        let mut water_face_triangles = Vec::<Vec<crate::ocean::MaskTriangle>>::new();
        for entry in crate::ocean::surfaces::collect(&vertices, &coarse_sources, &full_sources) {
            let source = entry.source;
            water_face_triangles.push(entry.triangles);
            water_face_keys.push(entry.key);
            water_face_sources.push(source);
            water_face_appearances.push(if source.authored_ocean.is_none() {
                diffuse_stages.get(&(source.vertices.start, source.vertices.end))
                    .map(|stage| crate::ocean::appearance::from_stage(stage, &vertices, &texture_data))
            } else { None });
            water_faces.push(entry.face);
        }
        let sky_triangles: Vec<crate::ocean::sky_extend::SkyTriangle> = coarse_sources.iter()
            .filter(|s| s.pipeline.class == DrawClass::Sky)
            .flat_map(|source| vertices[source.vertices.start as usize..source.vertices.end as usize]
                .chunks_exact(3)
                .map(|triangle| [triangle[0].position,triangle[1].position,triangle[2].position])
                .collect::<Vec<_>>())
            .collect();
        let footprints = crate::ocean::build_water_footprints(
            &water_faces,
            &water_face_triangles,
            crate::ocean::WATER_FOOTPRINT_TILE_SIZE,
        );
        if footprints.len() > MAX_OCEAN_SURFACES {
            rverbose!(
                1,
                "Ocean: {} promoted water surfaces from {} faces; the {} smallest stay on their authored faces",
                footprints.len(),
                water_faces.len(),
                footprints.len() - MAX_OCEAN_SURFACES,
            );
        }
        // Preserve the actual map-load selection decision for every candidate.
        // Both chosen and over-cap faces use the same keys as ocean_clipmaps,
        // so Trace never has to infer a rank from shader names or bounds.
        for (index, footprint) in footprints.iter().enumerate() {
            let decision = OceanPromotionDecision {
                rank: index + 1,
                total: footprints.len(),
                area: footprint.area(),
                faces: footprint.members.len(),
                slot: (index < MAX_OCEAN_SURFACES).then_some(index as u8),
            };
            for &member in &footprint.members {
                ocean_promotion_decisions.insert(water_face_keys[member], decision);
            }
        }
        for (slot, footprint) in footprints.iter().take(MAX_OCEAN_SURFACES).enumerate() {
            let authored = water_faces[footprint.members[0]].authored_ocean.is_some();
            // Use one stable representative for every face sharing this clipmap.
            // Choosing its largest member avoids PVS pieces changing tint,
            // tcMod-derived swell direction, or calm-water LOD by submission order.
            let representative = footprint.members.iter().copied().max_by(|&a, &b| {
                let area = |i: usize| {
                    let f = water_faces[i];
                    (f.maximum[0]-f.minimum[0]) * (f.maximum[1]-f.minimum[1])
                };
                area(a).total_cmp(&area(b))
            }).unwrap_or(footprint.members[0]);
            let mut appearance = water_face_appearances[representative];
            let mut promoted_source = None;
            if !authored {
                let primary = water_face_sources[representative];
                let source = diffuse_stages
                    .get(&(primary.vertices.start, primary.vertices.end))
                    .unwrap_or(primary);
                appearance = Some(appearance.unwrap_or_else(||
                    crate::ocean::appearance::from_stage(source, &vertices, &texture_data)));
                promoted_source = Some(source);
            }

            // Phase 1: extend only where real water triangles touch a vertical
            // BSP sky wall at the same height. Never derive this from coastal
            // banks, bathymetry or the shoreline solver.
            let sky_candidate = (!authored).then(||
                crate::ocean::sky_extend::detect(footprint, &sky_triangles)
            ).flatten();
            // Keep two fully independent masks, so r_oceanSkyExtend 0 restores
            // the exact original source-water outline without a map reload.
            let base_needs_mask = !authored && crate::ocean::promoted_outline_needs_mask(
                footprint.minimum, footprint.maximum, &footprint.triangles,
            );
            let base_masked = if authored {
                false
            } else if base_needs_mask {
                ocean_masks.push_footprint(slot, footprint)
            } else {
                ocean_masks.set_footprint(slot, footprint);
                false
            };
            // Source water outline/mask stays exactly as authored. Sky contact
            // intervals are passed separately to the ocean shader; no finite
            // synthetic mask or coastal footprint is ever generated.
            if let Some(sky) = sky_candidate {
                eprintln!("OCEAN SKY phase1: pool {} has {} sky contacts (camera-relative)",
                    slot + 1, sky.count);
                ocean_sky_regions[slot] = Some(sky);
            }
            let mask_slot = (base_masked || sky_candidate.is_some()).then_some(slot);
            // Reserve the existing eight clipmap grids for a camera-relative
            // extension. Baseline OFF still uses its exact footprint-based LOD.
            // Both mesh qualities are uploaded so the quality setting stays a
            // live switch instead of needing a map reload. Promoted water uses
            // upload-once vertex grids plus rewriteable index ranges: amplitude
            // overrides and true 3D camera distance can therefore change the
            // ring topology live instead of being frozen at map upload.
            let ranges = if let Some(appearance) = appearance {
                let mut low = crate::ocean::build_runtime_promoted_clipmap(
                    crate::ocean::OCEAN_LOW_MESH_SPACING_UNITS,
                    footprint.plane, footprint.minimum, footprint.maximum, mask_slot,
                    appearance.heuristic.amplitude,
                );
                let mut high = crate::ocean::build_runtime_promoted_clipmap(
                    crate::ocean::OCEAN_HIGH_MESH_SPACING_UNITS,
                    footprint.plane, footprint.minimum, footprint.maximum, mask_slot,
                    appearance.heuristic.amplitude,
                );
                if sky_candidate.is_some() {
                    // The grids already contain all eight possible levels.
                    // Reserve a full camera-relative index range without
                    // injecting INFINITY into the actual vertex attributes.
                    for clipmap in [&mut low, &mut high] {
                        clipmap.layout.extent = f32::INFINITY;
                        clipmap.index_capacity = clipmap.layout.maximum_index_count();
                    }
                }
                ocean_coarsest_spacing = low.coarsest_spacing;
                upload_timings.ocean_clipmap_vertices +=
                    (low.vertices.len() + high.vertices.len()) as u64;
                upload_timings.ocean_clipmap_tris += (high.indices.len() / 3) as u64;
                upload_timings.ocean_clipmap_spacing = crate::ocean::OCEAN_HIGH_MESH_SPACING_UNITS;
                upload_timings.ocean_coarsest_spacing = low.coarsest_spacing;

                let mut append = |quality: usize, clipmap: crate::ocean::RuntimePromotedClipmap| {
                    let half_cells = crate::ocean::promoted_clipmap_half_cells(appearance.heuristic.amplitude);
                    let (active_range, reserved_range, vertex_base) = indexed_builder.append_runtime_clipmap(
                        &clipmap.vertices, &clipmap.indices, clipmap.index_capacity,
                    );
                    runtime_ocean_clipmaps.push(RuntimeOceanClipmap {
                        slot: slot as u8,
                        quality,
                        layout: clipmap.layout,
                        vertex_base,
                        reserved_range,
                        active_range: active_range.clone(),
                        current_half_cells: half_cells,
                        current_minimum_level: 0,
                        current_level_limit: u32::MAX,
                        current_centre: [f32::NAN; 2],
                        current_pvs_cluster: None,
                        current_sky_extend: false,
                    });
                    active_range
                };
                [append(0, low), append(1, high)]
            } else {
                let low = crate::ocean::build_clipmap(
                    crate::ocean::OCEAN_LOW_MESH_SPACING_UNITS,
                    footprint.plane, footprint.minimum, footprint.maximum, mask_slot,
                );
                let high = crate::ocean::build_clipmap(
                    crate::ocean::OCEAN_HIGH_MESH_SPACING_UNITS,
                    footprint.plane, footprint.minimum, footprint.maximum, mask_slot,
                );
                ocean_coarsest_spacing = low.coarsest_spacing;
                upload_timings.ocean_clipmap_vertices +=
                    (low.vertices.len() + high.vertices.len()) as u64;
                upload_timings.ocean_clipmap_tris += (high.indices.len() / 3) as u64;
                upload_timings.ocean_clipmap_spacing = crate::ocean::OCEAN_HIGH_MESH_SPACING_UNITS;
                upload_timings.ocean_coarsest_spacing = low.coarsest_spacing;
                [
                    indexed_builder.append_clipmap(&low.vertices, &low.indices),
                    indexed_builder.append_clipmap(&high.vertices, &high.indices),
                ]
            };
            if let Some(source) = promoted_source {
                let appearance = appearance.expect("promoted water appearance");
                let name = source.texture.and_then(|i| texture_data.get(i))
                    .map(|t| t.label.clone()).unwrap_or_else(|| format!("water shader {}", source.bsp_shader_index));
                upload_timings.promoted_oceans.push(crate::ocean::appearance::PromotedOcean {
                    index: slot, name,
                    surface: crate::ocean::OceanSurface { plane_height:footprint.plane, minimum:footprint.minimum, maximum:footprint.maximum },
                    heuristic:appearance.heuristic,
                    roughness:(appearance.material[3] >= 0.0).then_some(appearance.material[3]),
                    color:crate::ocean::appearance::linear_to_srgb([appearance.color[0],appearance.color[1],appearance.color[2]]),
                    swell_direction:crate::ocean::appearance::swell_direction_from_stage(source, &vertices),
                });
            }
            for &member in &footprint.members {
                if let Some(appearance) = appearance {
                    ocean_appearances.insert(water_face_keys[member], appearance);
                }
                ocean_clipmaps.insert(water_face_keys[member], (ranges.clone(), slot as u8));
            }
            ocean_surfaces.push(crate::ocean::OceanSurface {
                plane_height: footprint.plane,
                minimum: footprint.minimum,
                maximum: footprint.maximum,
            });
        }
    }
    if build_ocean_meshes {
        let extended = ocean_sky_regions.iter().filter(|region| region.is_some()).count();
        eprintln!("OCEAN SKY phase1: {} / {} pools have sky contacts; r_oceanSkyExtend 0 baseline, 1 camera-relative clipmap, coastal untouched",
            extended, ocean_surfaces.len());
    }
    let _ = ocean_coarsest_spacing;
    if build_ocean_meshes {
        rverbose!(2, "[MAP OCEAN GEOMETRY] CPU build {:.1} ms | {} grid vertices (low+high) | {} pools | {} dynamic clipmaps (does not include GPU execution or first render)",
            ocean_mesh_started.elapsed().as_secs_f64() * 1000.0,
            upload_timings.ocean_clipmap_vertices,
            ocean_surfaces.len(), runtime_ocean_clipmaps.len());
    }
    upload_timings.ocean_water_batches = coarse_sources
        .iter()
        .chain(full_sources.iter())
        .filter(|source| source.water_primary && is_upward_water_face(source, &vertices))
        .count() as u32;

    let static_ao_source_to_gpu = indexed_builder
        .source_to_index
        .iter()
        .map(|index| index.unwrap_or(u32::MAX))
        .collect::<Vec<_>>();
    let static_ao_gpu_vertices = indexed_builder.vertices.clone();
    // Inline models go after the static-AO snapshot (AO re-uploads exactly
    // that prefix) and bypass vertex interning, so transforming a mover can
    // never drag a coincident world vertex along with it.
    let mut inline_index_ranges = vec![0..0; inline_batch_sources.len()];
    let mut inline_gpu_starts = Vec::with_capacity(inline_model_sources.len());
    for model in &inline_model_sources {
        let gpu_start = u32::try_from(indexed_builder.vertices.len()).unwrap_or(u32::MAX);
        let source = model.vertices.start as usize..model.vertices.end as usize;
        let inline_count = source.end.saturating_sub(source.start);
        indexed_builder
            .vertices
            .extend_from_slice(&inline_vertices[source]);
        indexed_builder
            .legacy_dlight_surface_ids
            .extend(std::iter::repeat(u32::MAX).take(inline_count));
        for batch_index in model.batches.clone() {
            let batch = &inline_batch_sources[batch_index];
            let indices = batch
                .vertices
                .clone()
                .map(|vertex| gpu_start + (vertex - model.vertices.start))
                .collect::<Vec<_>>();
            inline_index_ranges[batch_index] = indexed_builder.append_indices(&indices);
        }
        inline_gpu_starts.push(gpu_start);
    }
    let mut world_vertex_usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
    if ray_tracing_supported {
        world_vertex_usage |= wgpu::BufferUsages::BLAS_INPUT;
    }
    let vertex_gpu_started = Instant::now();
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA indexed static world vertices"),
        contents: bytemuck::cast_slice(&indexed_builder.vertices),
        usage: world_vertex_usage,
    });
    let legacy_surface_ids = if indexed_builder.legacy_dlight_surface_ids.is_empty() {
        vec![u32::MAX]
    } else {
        indexed_builder.legacy_dlight_surface_ids.clone()
    };
    let legacy_dlight_surface_id_buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA Legacy dlight BSP surface ids"),
            contents: bytemuck::cast_slice(&legacy_surface_ids),
            usage: wgpu::BufferUsages::VERTEX,
        });
    vertex_gpu_calls_ms += vertex_gpu_started.elapsed().as_secs_f64() * 1000.0;
    let static_index_count=indexed_builder.indices.len();
    let mut patch_lod = super::patch_lod::WorldPatchLod::prepare(
        &mut indexed_builder, &patch_draws, &inline_patch_draws,
        coarse_sources.iter().zip(&coarse_index_ranges).chain(full_sources.iter().zip(&full_index_ranges)).map(|(s,r)|(s.vertices.clone(),r.clone())),
        inline_batch_sources.iter().zip(&inline_index_ranges).map(|(s,r)|(s.vertices.clone(),r.clone())),
        &inline_model_sources, &inline_gpu_starts, &legacy_dlight_triangle_surfaces,
    );
    // After patch LOD has snapshotted its immutable source. Every compiled-model
    // LOD is an additional index-only range, never a replacement of original
    // geometry (AUTO4, marks, RT, and secondary views retain the original).
    let mut bsp_model_lod = super::bsp_model_lod::BspModelLod::prepare(
        &mut indexed_builder,
        &coarse_sources, &coarse_index_ranges,
        &full_sources, &full_index_ranges,
        &bsp_compiled_model_surfaces, &patch_draws,
    );
    let auto4_source_indices = Arc::new(std::mem::take(&mut indexed_builder.indices));
    patch_lod.finish(Arc::clone(&auto4_source_indices));
    bsp_model_lod.finish(Arc::clone(&auto4_source_indices));
    let base_index_count = u32::try_from(auto4_source_indices.len()).unwrap_or(u32::MAX);
    let base_index_bytes =
        (auto4_source_indices.len() as u64).saturating_mul(std::mem::size_of::<u32>() as u64);
    // Never reserve more than building every non-contiguous recipe would need.
    let requested_auto4_cache_bytes = (portal_draw_plan.packed_index_count as u64 + if patch_draws.is_empty() {0} else {full_index_ranges.iter().map(|r|u64::from(r.end-r.start)).sum::<u64>()} + bsp_model_lod.auto4_extra_indices())
        .saturating_mul(std::mem::size_of::<u32>() as u64)
        .min(AUTO4_COLLAPSE_CACHE_BYTES);
    let max_extra_bytes = device
        .limits()
        .max_buffer_size
        .saturating_sub(base_index_bytes)
        .min(
            u64::from(u32::MAX.saturating_sub(base_index_count))
                .saturating_mul(std::mem::size_of::<u32>() as u64),
        );
    let auto4_cache_bytes = requested_auto4_cache_bytes.min(max_extra_bytes)
        / std::mem::size_of::<u32>() as u64
        * std::mem::size_of::<u32>() as u64;
    let auto4_cache_indices =
        u32::try_from(auto4_cache_bytes / std::mem::size_of::<u32>() as u64).unwrap_or(0);
    let index_buffer_size = base_index_bytes
        .saturating_add(auto4_cache_bytes)
        .max(std::mem::size_of::<u32>() as u64);
    let mut world_index_usage = wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST;
    if ray_tracing_supported {
        world_index_usage |= wgpu::BufferUsages::BLAS_INPUT;
    }
    let index_gpu_started = Instant::now();
    let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("JKA static world indices + AUTO 4 collapse cache"),
        size: index_buffer_size,
        usage: world_index_usage,
        mapped_at_creation: false,
    });
    if !auto4_source_indices.is_empty() {
        queue.write_buffer(
            &index_buffer,
            0,
            bytemuck::cast_slice(&auto4_source_indices[..static_index_count]),
        );
    }
    vertex_gpu_calls_ms += index_gpu_started.elapsed().as_secs_f64() * 1000.0;
    let source_vertex_count = vertices.len();
    let indexed_vertex_count = indexed_builder.vertices.len();
    let reused_vertex_count = source_vertex_count.saturating_sub(indexed_vertex_count);
    rverbose!(
        1,
        "Indexed world geometry: {} triangle-list vertices -> {} unique vertices ({} reused, {:.1}% reduction)",
        source_vertex_count,
        indexed_vertex_count,
        reused_vertex_count,
        if source_vertex_count == 0 {
            0.0
        } else {
            reused_vertex_count as f64 * 100.0 / source_vertex_count as f64
        }
    );
    upload_timings.vertex_buffer_ms = vertex_started.elapsed().as_secs_f64() * 1000.0;
    upload_timings.vertex_gpu_calls_ms = vertex_gpu_calls_ms;
    upload_timings.vertex_cpu_ms = (upload_timings.vertex_buffer_ms - vertex_gpu_calls_ms).max(0.0);
    let texture_started = Instant::now();
    let texture_clamp: Vec<_> = texture_data.iter().map(|data| data.clamp).collect();
    let sky_covers_background = sky_outer_box_covers_background(
        coarse_sources.iter().chain(full_sources.iter()),
    );
    println!("r_colorClearMode: sky box covers the background: {}",
        if sky_covers_background { "yes (clear can be skipped)" } else { "no (clear kept)" });
    let sky_average_started = Instant::now();
    // This single RGB value feeds optional cloud/ocean lighting. Keep it
    // available for runtime toggles; sampling at most six texture faces is
    // inexpensive and no longer scales with PVS or draw batch count.
    let sky_average = average_skybox_color(&texture_data, primary_skybox);
    upload_timings.skybox_average_ms = sky_average_started.elapsed().as_secs_f64() * 1000.0;
    upload_timings.skybox_face_references = primary_skybox.map_or(0, |faces| faces.len());
    upload_timings.skybox_unique_images = primary_skybox.map_or(0, |faces| {
        faces.into_iter().collect::<BTreeSet<_>>().len()
    });
    let mut compressed_count = 0usize;
    let companion_bc3 = |index: usize, data: &TextureData| {
        compress_pbr_companions
            && !data.srgb
            && texture_usage.get(index).copied().unwrap_or_default() == TEX_USE_COMPANION
    };
    // TaystJK r_smartpicmip defaults on and disables picmip for any
    // image whose qpath is not under textures/. Keep that compatibility
    // rule rather than degrading videoMap/gfx/UI-style dependencies that
    // merely happen to be referenced by a world shader.
    let texture_picmip = |index: usize, data: &TextureData| {
        if texture_usage[index] & TEX_NO_PICMIP == 0 && data
            .label
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("textures/"))
        {
            picmip
        } else {
            0
        }
    };
    // Encoding began before geometry preparation. Only block here if workers
    // still have outstanding compression, and attribute that wait separately.
    let color_wait_started = Instant::now();
    let (encoded, encode_ms) = match color_encode_rx {
        Some(rx) => rx.recv().unwrap_or_else(|_| {
            eprintln!("World BC encoder worker failed; retrying on render thread");
            let started = Instant::now();
            let result = texture_compression::encode_world_textures_skipping(
                &texture_data, &texture_usage, picmip,
                texture_compression >= 1, compress_pbr_companions, &reuse_bitmap,
            );
            (result, started.elapsed().as_secs_f64() * 1000.0)
        }),
        None => (Vec::new(), 0.0),
    };
    upload_timings.texture_encode_wait_ms = color_wait_started.elapsed().as_secs_f64() * 1000.0;
    upload_timings.texture_encode_cpu_ms = encode_ms;
    let mut color_compressed = 0usize;
    let texture_gpu_started = Instant::now();
    let textures: Vec<_> = texture_data
        .iter()
        .enumerate()
        .map(|(index, data)| {
            if let Some(previous) = reusable_world.and_then(|world| {
                reusable_textures[index].and_then(|old_index| world.textures.get(old_index))
            }) {
                return previous.clone();
            }
            let texture_picmip = texture_picmip(index, data);
            if let Some(Some(encoded)) = encoded.get(index) {
                if companion_bc3(index, data) {
                    compressed_count += 1;
                } else {
                    color_compressed += 1;
                }
                texture_compression::upload_encoded_texture(device, queue, &data.label, encoded)
            } else {
                upload_texture_picmip(device, queue, data, texture_picmip)
            }
        })
        .collect();
    upload_timings.texture_gpu_calls_ms = texture_gpu_started.elapsed().as_secs_f64() * 1000.0;
    if texture_compression >= 1 {
        rverbose!(
            1,
            "r_ext_compress_textures {texture_compression}: BC-compressed {color_compressed}/{} map textures (encode {encode_ms:.1} ms)",
            texture_data.len()
        );
    }
    if compress_pbr_companions {
        rverbose!(
            2,
            "PBR companion BC: compressed {compressed_count}/{} uploaded map textures",
            texture_data.len()
        );
    }
    let footprint_marks: [&GpuImage; 2] = std::array::from_fn(|slot| {
        footprint_mark_textures[slot]
            .and_then(|index| textures.get(index))
            .unwrap_or(white)
    });
    let surface_sprite_effects = surface_sprite_effect_sources
        .into_iter()
        .filter_map(|source| {
            let texture = textures.get(source.texture).unwrap_or(missing);
            let sampler = if source.clamp {
                clamp_sampler
            } else {
                repeat_sampler
            };
            Some(SurfaceSpriteEffectGpu {
                bind_group: surface_sprite_effect_renderer.create_texture_bind_group(
                    device,
                    &texture.view,
                    sampler,
                ),
                source,
            })
        })
        .collect::<Vec<_>>();
    let surface_sprite_resident =
        surface_sprite_effect_renderer.build_resident(device, &surface_sprite_effects, &surface_sprite_bake);
    upload_timings.texture_upload_ms = texture_started.elapsed().as_secs_f64() * 1000.0;
    let lightmap_started = Instant::now();
    // r_ext_compress_textures 2 also takes 8-bit lightmaps to BC1; FP16
    // lightmaps (r_floatLightmap) are never compressed.
    let lightmap_wait_started = Instant::now();
    let (encoded_lightmaps, lightmap_encode_ms) = match lightmap_encode_rx {
        Some(rx) => rx.recv().unwrap_or_else(|_| {
            eprintln!("World BC lightmap encoder worker failed; retrying on render thread");
            let started = Instant::now();
            let result = texture_compression::encode_textures(&lightmap_data, |_| Some(0));
            (result, started.elapsed().as_secs_f64() * 1000.0)
        }),
        None => (Vec::new(), 0.0),
    };
    upload_timings.lightmap_encode_wait_ms = lightmap_wait_started.elapsed().as_secs_f64() * 1000.0;
    upload_timings.lightmap_encode_cpu_ms = lightmap_encode_ms;
    let lightmap_gpu_started = Instant::now();
    let lightmaps: Vec<_> = if let Some(previous) = reusable_lightmaps {
        previous.lightmaps.clone()
    } else {
        lightmap_data
        .iter()
        .enumerate()
        .map(|(index, data)| match encoded_lightmaps.get(index) {
            Some(Some(encoded)) => {
                texture_compression::upload_encoded_texture(device, queue, &data.label, encoded)
            }
            _ => upload_texture(device, queue, data),
        })
        .collect()
    };
    let deluxemaps: Vec<_> = if reusable_lightmaps.is_some_and(|previous|
        previous.deluxemaps.len() == deluxemap_data.len().max(1)) {
        reusable_lightmaps.unwrap().deluxemaps.clone()
    } else if deluxemap_data.is_empty() {
        let neutral = TextureData {
            label: "JKA neutral deluxemap fallback".into(),
            source: None,
            width: 1,
            height: 1,
            rgba: vec![127, 127, 127, 0],
            rgba16f: None,
            mip_level_count: 1,
            clamp: true,
            srgb: false,
        };
        vec![upload_texture(device, queue, &neutral)]
    } else {
        deluxemap_data
            .iter()
            .map(|data| upload_texture(device, queue, data))
            .collect()
    };
    upload_timings.lightmap_gpu_calls_ms = lightmap_gpu_started.elapsed().as_secs_f64() * 1000.0;
    upload_timings.lightmap_upload_ms = lightmap_started.elapsed().as_secs_f64() * 1000.0;
    let reflection_probe_started = Instant::now();
    let reflection_probes = upload_reflection_probes(device, queue, &reflection_probe_data);
    upload_timings.reflection_probe_upload_ms =
        reflection_probe_started.elapsed().as_secs_f64() * 1000.0;

    let mut cull_records = Vec::<GpuCullRecord>::new();
    let mut initial_indirect = Vec::<DrawIndexedIndirectArgs>::new();
    let mut material_arena = UniformArena::new(
        device,
        "JKA material uniforms",
        std::mem::size_of::<MaterialUniform>() as u64,
    );
    let mut fog_arena = UniformArena::new(
        device,
        "JKA surface fog uniforms",
        std::mem::size_of::<weather::fog::SurfaceFogUniform>() as u64,
    );
    let mut upload_batches = |sources: Vec<DrawBatch>,
                              index_ranges: Vec<std::ops::Range<u32>>,
                              source_vertices: &[GpuVertex],
                              triangle_surfaces: Option<&[u32]>| {
        let mut uploaded = Vec::with_capacity(sources.len());
        for (mut source, index_range) in sources.into_iter().zip(index_ranges) {
            let mut water_plane: Option<(f32, [f32; 2], [f32; 2])> = None;
            if source.water_primary {
                if is_upward_water_face(&source, source_vertices) {
                    water_plane = Some(water_surface_extent(&source, source_vertices));
                } else {
                    // A water brush contributes its underside and its sides
                    // too. Those are not the surface to promote. Preserve
                    // their authored draw unless a replacement owns them.
                    source.water_primary = false;
                }
            }
            // Ownership is independent of stage order or face orientation.
            // Coplanar undersides and all sibling stages share the replacement.
            let surface_key = source.water.then(|| {
                let (plane, min, max) = water_surface_extent(&source, source_vertices);
                ocean_surface_key(plane, min, max, source.bsp_shader_index)
            });
            // A demoted overlay sheet lying over a promoted ocean is a duplicate of
            // the surf and foam that ocean draws. Same suppression as a replaced
            // face, so it still returns when r_ocean is off.
            let overlay_covered = source.water_overlay
                && crate::ocean::surfaces::overlay_covered_by_ocean(
                    &source_vertices[source.vertices.start as usize..source.vertices.end as usize],
                    &ocean_surfaces,
                );
            let ocean_replaced = overlay_covered
                || surface_key.is_some_and(|key| ocean_clipmaps.contains_key(&key));
            let (ocean_clipmap, ocean_clipmap_id) =
                match water_plane.and_then(|(plane, minimum, maximum)| {
                    ocean_clipmaps
                        .get(&ocean_surface_key(plane, minimum, maximum, source.bsp_shader_index))
                        .cloned()
                }) {
                    Some((ranges, id)) => (Some(ranges), id),
                    None => (None, u8::MAX),
                };
            if let Some((plane, _, _)) = water_plane {
                source.planar_plane = [0.0, 1.0, 0.0, -plane];
            }
            let detail_auto_word = source
                .texture
                .and_then(|texture| detail_texture_by_base.get(texture).copied())
                .unwrap_or(0);
            let mut material_uniform =
                material_uniform(&source, (detail_auto_word & AUTO_DETAIL_ENABLED_BIT) != 0);
            if let Some(appearance) = surface_key.and_then(|key| ocean_appearances.get(&key)) {
                material_uniform.ocean_color = appearance.color;
                material_uniform.ocean_material = appearance.material;
                material_uniform.ocean_waves = appearance.waves;
                let own = crate::ocean::appearance::from_stage(&source,source_vertices,&texture_data);
                material_uniform.ocean_blend_color = own.blend_color;
            }
            let fragment_key =
                MaterialFragmentKey::from_uniform(&material_uniform);
            let material_buffer =
                material_arena.push(device, queue, bytemuck::bytes_of(&material_uniform));
            // Extra authored stage descriptors share the existing material uniform
            // arena. Ordinary materials alias the base slot (zero extra uploads).
            let additive_uniforms: [UniformSlot; 2] = std::array::from_fn(|i| {
                source.additive_companions.get(i).map_or_else(
                    || material_buffer.clone(),
                    |companion| {
                        let mut aux = crate::renderer::material_uniform(companion, false);
                        aux.params[3] = source.shader_time_offset;
                        aux.vector_t[3] = source.shader_clamp_time.unwrap_or(0.0);
                        if companion.texture.and_then(|idx| texture_clamp.get(idx)).copied().unwrap_or(false) {
                            aux.header[3] |= 65536; // separate repeat/clamp sampler for each extra stage
                        }
                        material_arena.push(device, queue, bytemuck::bytes_of(&aux))
                    },
                )
            });
            let fog_uniform = weather::fog::surface_fog_uniform(
                source.fog,
                source.fog_is_global,
                source.fog_color_override,
                source.legacy2_fog_in_stage_safe,
                source.global_fog_post_eligible,
            );
            let fog_buffer = fog_arena.push(device, queue, bytemuck::bytes_of(&fog_uniform));
            // OpenJK Dynamic Glow renders only `glow` stages and temporarily
            // overrides fog colour to black. Preserve the stage's ordinary
            // visibility/fog amount while preventing the fog colour itself
            // from becoming an emissive source. This tiny alternate uniform is
            // created only for authored glow batches; no per-frame cost exists
            // for ordinary materials or Glow Off.
            let authored_glow_fog_buffer = source.authored_glow.then(|| {
                let glow_fog_uniform = weather::fog::surface_fog_uniform(
                    source.fog,
                    source.fog_is_global,
                    weather::fog::FogColorOverride::Black,
                    source.legacy2_fog_in_stage_safe,
                    source.global_fog_post_eligible,
                );
                fog_arena.push(device, queue, bytemuck::bytes_of(&glow_fog_uniform))
            });
            let selected_detail_texture = detail_texture_for_source(
                &source,
                detail_texture,
                detail_texture_auto,
                detail_auto_textures,
                &detail_texture_by_base,
            );
            let bind_group = create_world_bind_group(
                device,
                surface_layout,
                white,
                missing,
                flat_normal,
                repeat_sampler,
                clamp_sampler,
                pbr_repeat_sampler,
                pbr_clamp_sampler,
                lightmap_sampler,
                sky_sampler,
                selected_detail_texture,
                &textures,
                &lightmaps,
                &deluxemaps,
                &texture_clamp,
                primary_skybox,
                &footprint_marks,
                &reflection_probes,
                &source,
                &material_buffer,
                &additive_uniforms,
                &fog_buffer,
                surface_deformation_buffer,
                deformation_field_views,
                deformation_field_sampler,
            );
            // Keep the stock material/binding intact for Ocean Off. The ocean
            // pass may need a later diffuse stage instead of the primary stage's
            // environment reflection texture and generators.
            let ocean_material = if ocean_clipmap.is_some() && source.authored_ocean.is_none() {
                surface_key
                    .and_then(|key| ocean_appearances.get(&key))
                    .map(|appearance| {
                        let stage = diffuse_stages
                            .get(&(source.vertices.start, source.vertices.end))
                            .unwrap_or(&source);
                        let own =
                            crate::ocean::appearance::from_stage(stage, source_vertices, &texture_data);
                        let mut uniform = crate::renderer::material_uniform(stage, false);
                        // The diffuse stage never received the plane resolved
                        // for `source` above, and ocean_environment matches the
                        // planar reflection slot against this uniform's plane.
                        uniform.planar_plane = source.planar_plane;
                        uniform.ocean_color = appearance.color;
                        uniform.ocean_material = appearance.material;
                        uniform.ocean_waves = [
                            1.0,
                            appearance.heuristic.amplitude,
                            own.waves[2],
                            own.waves[3],
                        ];
                        // Every member shares a stable mean-color/lighting
                        // estimate. Source texture detail is used only to choose
                        // wave defaults and never sampled by the ocean shader.
                        uniform.ocean_blend_color = appearance.blend_color;
                        let buffer = material_arena.push(device, queue, bytemuck::bytes_of(&uniform));
                        (stage, buffer)
                    })
            } else {
                None
            };
            let ocean_surface_bind_group = ocean_material.as_ref().map(|(stage, buffer)| {
                // The six sky slots double as additive-companion and legacy
                // overlay textures. The FFT ocean never draws those stages, but
                // its environment reflection (`sample_map_sky_renderer`) reads
                // the slots as the map's skybox faces, so a companion left in
                // slot 0 replaces the whole +X face with a water texture.
                let mut sky_stage = (*stage).clone();
                sky_stage.additive_companions.clear();
                sky_stage.legacy_srgb_overlay = None;
                let stage = &sky_stage;
                create_world_bind_group(
                    device,
                    surface_layout,
                    white,
                    missing,
                    flat_normal,
                    repeat_sampler,
                    clamp_sampler,
                    pbr_repeat_sampler,
                    pbr_clamp_sampler,
                    lightmap_sampler,
                    sky_sampler,
                    selected_detail_texture,
                    &textures,
                    &lightmaps,
                    &deluxemaps,
                    &texture_clamp,
                    primary_skybox,
                    &footprint_marks,
                    &reflection_probes,
                    stage,
                    buffer,
                    &additive_uniforms,
                    &fog_buffer,
                    surface_deformation_buffer,
                    deformation_field_views,
                    deformation_field_sampler,
                )
            });
            let ocean_material_buffer = ocean_material.map(|(_, buffer)| buffer);
            let authored_glow_bind_group = authored_glow_fog_buffer.as_ref().map(|glow_fog_buffer| {
                create_world_bind_group(
                    device,
                    surface_layout,
                    white,
                    missing,
                    flat_normal,
                    repeat_sampler,
                    clamp_sampler,
                    pbr_repeat_sampler,
                    pbr_clamp_sampler,
                    lightmap_sampler,
                    sky_sampler,
                    selected_detail_texture,
                    &textures,
                    &lightmaps,
                    &deluxemaps,
                    &texture_clamp,
                    primary_skybox,
                    &footprint_marks,
                    &reflection_probes,
                    &source,
                    &material_buffer,
                &additive_uniforms,
                    glow_fog_buffer,
                    surface_deformation_buffer,
                    deformation_field_views,
                    deformation_field_sampler,
                )
            });
            let fast_bind_group = create_fast_world_bind_group(
                device,
                fast_surface_layout,
                white,
                missing,
                repeat_sampler,
                clamp_sampler,
                lightmap_sampler,
                sky_sampler,
                &textures,
                &lightmaps,
                &texture_clamp,
                primary_skybox,
                &footprint_marks,
                &source,
                &material_buffer,
                &additive_uniforms,
                surface_deformation_buffer,
                deformation_field_views,
                deformation_field_sampler,
            );
            let animation = build_world_animation(
                &source, &bind_group, &fast_bind_group, &authored_glow_bind_group,
                |frame_source| WorldAnimationFrame {
                    full: create_world_bind_group(
                        device, surface_layout, white, missing, flat_normal,
                        repeat_sampler, clamp_sampler, pbr_repeat_sampler, pbr_clamp_sampler,
                        lightmap_sampler, sky_sampler, selected_detail_texture,
                        &textures, &lightmaps, &deluxemaps, &texture_clamp,
                        primary_skybox, &footprint_marks, &reflection_probes,
                        frame_source, &material_buffer, &additive_uniforms, &fog_buffer,
                        surface_deformation_buffer, deformation_field_views, deformation_field_sampler,
                    ),
                    fast: create_fast_world_bind_group(
                        device, fast_surface_layout, white, missing, repeat_sampler,
                        clamp_sampler, lightmap_sampler, sky_sampler, &textures, &lightmaps,
                        &texture_clamp, primary_skybox, &footprint_marks, frame_source,
                        &material_buffer, &additive_uniforms, surface_deformation_buffer,
                        deformation_field_views, deformation_field_sampler,
                    ),
                    glow: authored_glow_fog_buffer.as_ref().map(|glow_fog_buffer| {
                        create_world_bind_group(
                            device, surface_layout, white, missing, flat_normal,
                            repeat_sampler, clamp_sampler, pbr_repeat_sampler, pbr_clamp_sampler,
                            lightmap_sampler, sky_sampler, selected_detail_texture,
                            &textures, &lightmaps, &deluxemaps, &texture_clamp,
                            primary_skybox, &footprint_marks, &reflection_probes,
                            frame_source, &material_buffer, &additive_uniforms, glow_fog_buffer,
                            surface_deformation_buffer, deformation_field_views,
                            deformation_field_sampler,
                        )
                    }),
                },
            );
            let cull_index = u32::try_from(cull_records.len()).unwrap_or(u32::MAX);
            let record = make_cull_record(&source, source_vertices, &index_range);
            initial_indirect.push(DrawIndexedIndirectArgs {
                index_count: record.draw[0],
                instance_count: 1,
                first_index: record.draw[1],
                base_vertex: 0,
                first_instance: 0,
            });
            let mut bounds_min = [record.minimum[0], record.minimum[1], record.minimum[2]];
            let mut bounds_max = [record.maximum[0], record.maximum[1], record.maximum[2]];
            if source.water_primary {
                // FFT displacement moves the promoted plane vertically beyond its
                // authored BSP bounds. Keep CPU/frustum visibility conservative so
                // wave crests do not disappear at the edge of the view.
                bounds_min[1] -= 256.0;
                bounds_max[1] += 256.0;
            }
            let legacy_dlight_runs =
                legacy_dlight_runs_for_batch(&source, &index_range, triangle_surfaces);
            // Use original BSP surface planes only for a wholly coplanar,
            // one-sided draw range. Mixed/PVS-packed surfaces remain visible.
            let face_cull_plane = packed_face_plane(
                &source, &legacy_dlight_runs, &legacy_dlight_surfaces, source_vertices,
            );
            let face_cull_runs = authored_face_runs(
                &source, &index_range, &legacy_dlight_runs, &legacy_dlight_surfaces, source_vertices,
            );
            let mut record = record;
            record.face_plane = face_cull_plane;
            cull_records.push(record);
            uploaded.push(WorldBatch {
                full_indexed_range: index_range.clone(),
                patch_geometry: None,
                patch_suppressed: false,
                indexed_range: index_range,
                legacy_dlight_runs,
                face_cull_runs: face_cull_runs.into(),
                face_cull_plane,
                is_inline_entity: false,
                inline_model_slot: u32::MAX,
                inline_group: u32::MAX,
                ocean_clipmap,
                ocean_clipmap_id,
                ocean_replaced,
                source,
                bind_group,
                fragment_key,
                animation,
                authored_glow_bind_group,
                ocean_surface_bind_group,
                _ocean_material_buffer: ocean_material_buffer,
                fast_bind_group,
                _material_buffer: material_buffer,
                additive_uniforms,
                _fog_buffer: fog_buffer,
                _authored_glow_fog_buffer: authored_glow_fog_buffer,
                cull_index,
                compact_group: None,
                bounds_min,
                bounds_max,
            });
        }
        uploaded
    };

    let batch_started = Instant::now();
    let mut coarse_batches = upload_batches(
        coarse_sources,
        coarse_index_ranges,
        &vertices,
        Some(&legacy_dlight_triangle_surfaces),
    );
    let mut full_batches = if visibility.is_some() {
        upload_batches(
            full_sources,
            full_index_ranges,
            &vertices,
            Some(&legacy_dlight_triangle_surfaces),
        )
    } else {
        Vec::new()
    };
    // Keep one compact footprint per exact FULL/PVS water piece. At runtime we
    // union only the pieces already selected by the current cluster's PVS list;
    // no second visibility system or per-cluster ocean allocation is needed.
    let mut ocean_pvs_footprints =
        crate::ocean::OceanPvsFootprints::with_batch_count(full_batches.len());
    for (batch_index, batch) in full_batches.iter().enumerate() {
        let slot = batch.ocean_clipmap_id as usize;
        let Some(surface) = ocean_surfaces.get(slot).copied() else {
            continue;
        };
        if batch.ocean_clipmap.is_none() || !batch.source.water_primary {
            continue;
        }
        let source = batch.source.vertices.start as usize..batch.source.vertices.end as usize;
        let triangles = vertices[source].chunks_exact(3).map(|triangle| {
            [0, 1, 2].map(|vertex| {
                [
                    triangle[vertex].position[0],
                    triangle[vertex].position[2],
                ]
            })
        });
        ocean_pvs_footprints.add_piece(batch_index, slot, surface, triangles);
    }
    let planar_reflectors = collect_planar_reflectors(&coarse_batches, &vertices);
    // Inline models join both batch sets after the static world, so every
    // static batch index (reflectors, snow, AO) is unchanged. Their empty PVS
    // signature keeps them in every cluster's list.
    let inline_coarse_start = coarse_batches.len();
    let mut rt_shadow_ranges = Vec::<std::ops::Range<u32>>::new();
    let mut rt_shadow_surfaces = BTreeSet::<(u32, u32)>::new();
    for batch in &coarse_batches[..inline_coarse_start] {
        // Glass must not be duplicated in the opaque BLAS (even if its author
        // used an opaque render stage plus `surfaceparm lightfilter`).
        if crate::renderer::lighting::glass_transmission::classify_glass(
                &batch.source,
                batch.source.texture.and_then(|i| texture_data.get(i)).map(|t| t.label.as_str()),
                None, true,
            ).is_some() { continue; }
        if batch.source.pipeline.class != DrawClass::Opaque
            || batch.source.pipeline.blend != BlendMode::Opaque
            || batch.indexed_range.is_empty()
        {
            continue;
        }
        // A Q3 shader can render the same BSP surface in several stages. The
        // indexed builder gives those stages distinct index ranges, but putting
        // every copy in the BLAS would trace duplicate coplanar triangles. Keep
        // one opaque copy of each authored surface.
        let surface_key = (batch.source.vertices.start, batch.source.vertices.end);
        if !rt_shadow_surfaces.insert(surface_key) {
            continue;
        }
        // Merge adjacent opaque index runs. This reduces BLAS geometry count
        // without ever spanning a skipped sky/mask/translucent stage.
        if let Some(last) = rt_shadow_ranges.last_mut() {
            if last.end == batch.indexed_range.start {
                last.end = batch.indexed_range.end;
                continue;
            }
        }
        rt_shadow_ranges.push(batch.indexed_range.clone());
    }
    // Vulkan ray-query alpha testing: only surfaces without an opaque depth-
    // writing sibling need a non-opaque candidate geometry. This mirrors the
    // raster depth semantics and avoids turning decorative mask stages layered
    // over an opaque base into holes in the RT caster.
    let rt_opaque_surfaces = rt_shadow_surfaces.clone();
    let mut rt_mask_surfaces = BTreeSet::<(u32, u32)>::new();
    let mut rt_shadow_masks = Vec::<RtAlphaMaskSource>::new();
    for batch in &coarse_batches[..inline_coarse_start] {
        let glass = crate::renderer::lighting::glass_transmission::classify_glass(
            &batch.source,
            batch.source.texture.and_then(|i| texture_data.get(i)).map(|t| t.label.as_str()),
            None, true,
        ).is_some();
        if !glass && (batch.source.pipeline.class != DrawClass::Mask
            || batch.source.pipeline.blend != BlendMode::Opaque
            || !batch.source.pipeline.depth_write)
            || batch.source.vertices.is_empty()
        {
            continue;
        }
        let surface_key = (batch.source.vertices.start, batch.source.vertices.end);
        if rt_opaque_surfaces.contains(&surface_key) || !rt_mask_surfaces.insert(surface_key) {
            continue;
        }
        let texture = batch
            .source
            .texture
            .and_then(|index| texture_data.get(index));
        let caster = if glass {
            rt_glass_source_from_gpu_vertices(&vertices, &batch.source, texture)
        } else {
            rt_alpha_mask_source_from_gpu_vertices(&vertices, &batch.source, texture)
        };
        if let Some(mask) = caster { rt_shadow_masks.push(mask); }
    }
    let rt_vertex_count = u32::try_from(indexed_builder.vertices.len()).unwrap_or(u32::MAX);
    let glass_texture_names = texture_data.iter().map(|texture| texture.label.clone()).collect();
    let raster_glass_static = crate::renderer::lighting::glass_transmission::raster_glass_triangles(&rt_shadow_masks);
    // Preserve one object-space opaque triangle list per inline BSP model for
    // hardware RT. Raster movers continue to rewrite their private region of
    // the shared world vertex buffer; RT instead builds this local mesh once and
    // moves it with a TLAS transform, matching the Vulkan transform-only dynamic
    // object model.
    let inline_rt_indices = inline_model_sources
        .iter()
        .map(|model| {
            let mut seen_surfaces = BTreeSet::<(u32, u32)>::new();
            let mut indices = Vec::<u32>::new();
            for batch_index in model.batches.clone() {
                let Some(batch) = inline_batch_sources.get(batch_index) else {
                    continue;
                };
                if crate::renderer::lighting::glass_transmission::classify_glass(
                    batch,
                    batch.texture.and_then(|i| texture_data.get(i)).map(|t| t.label.as_str()),
                    None, true,
                ).is_some() { continue; }
                if batch.pipeline.class != DrawClass::Opaque
                    || batch.pipeline.blend != BlendMode::Opaque
                    || batch.vertices.is_empty()
                {
                    continue;
                }
                let surface_key = (batch.vertices.start, batch.vertices.end);
                if !seen_surfaces.insert(surface_key) {
                    continue;
                }
                indices.extend(
                    batch
                        .vertices
                        .clone()
                        .filter_map(|vertex| vertex.checked_sub(model.vertices.start)),
                );
            }
            indices
        })
        .collect::<Vec<_>>();
    let inline_rt_masks = inline_model_sources
        .iter()
        .map(|model| {
            let opaque_surfaces = model
                .batches
                .clone()
                .filter_map(|batch_index| inline_batch_sources.get(batch_index))
                .filter(|batch| {
                    crate::renderer::lighting::glass_transmission::classify_glass(
                        batch,
                        batch.texture.and_then(|i| texture_data.get(i)).map(|t| t.label.as_str()),
                        None, true,
                    ).is_none() && batch.pipeline.class == DrawClass::Opaque
                        && batch.pipeline.blend == BlendMode::Opaque
                        && batch.pipeline.depth_write
                })
                .map(|batch| (batch.vertices.start, batch.vertices.end))
                .collect::<BTreeSet<_>>();
            let mut seen_masks = BTreeSet::<(u32, u32)>::new();
            let mut masks = Vec::<RtAlphaMaskSource>::new();
            for batch_index in model.batches.clone() {
                let Some(batch) = inline_batch_sources.get(batch_index) else {
                    continue;
                };
                let glass = crate::renderer::lighting::glass_transmission::classify_glass(
                    batch,
                    batch.texture.and_then(|i| texture_data.get(i)).map(|t| t.label.as_str()),
                    None, true,
                ).is_some();
                if (!glass && (batch.pipeline.class != DrawClass::Mask
                    || batch.pipeline.blend != BlendMode::Opaque
                    || !batch.pipeline.depth_write))
                    || batch.vertices.is_empty()
                {
                    continue;
                }
                let surface_key = (batch.vertices.start, batch.vertices.end);
                if opaque_surfaces.contains(&surface_key) || !seen_masks.insert(surface_key) {
                    continue;
                }
                let texture = batch.texture.and_then(|index| texture_data.get(index));
                let caster = if glass {
                    rt_glass_source_from_gpu_vertices(&inline_vertices, batch, texture)
                } else {
                    rt_alpha_mask_source_from_gpu_vertices(&inline_vertices, batch, texture)
                };
                if let Some(mask) = caster { masks.push(mask); }
            }
            masks
        })
        .collect::<Vec<_>>();
    let inline_batch_count = inline_batch_sources.len();
    coarse_batches.extend(upload_batches(
        inline_batch_sources.clone(),
        inline_index_ranges.clone(),
        &inline_vertices,
        None,
    ));
    let inline_full_start = full_batches.len();
    if visibility.is_some() {
        full_batches.extend(upload_batches(
            inline_batch_sources,
            inline_index_ranges,
            &inline_vertices,
            None,
        ));
    }
    // Map every authored inline stage to its *render-instance slot*, not to
    // its BSP model handle. A shared handle can appear in multiple mover slots;
    // only the absent slot may be culled, without hiding its active sibling.
    let mut inline_slot_for_batch = vec![u32::MAX; inline_batch_count];
    for (slot, model) in inline_model_sources.iter().enumerate() {
        for index in model.batches.clone() {
            if let Some(mapped) = inline_slot_for_batch.get_mut(index) {
                *mapped = slot as u32;
            }
        }
    }
    for (index, batch) in coarse_batches[inline_coarse_start..].iter_mut().enumerate() {
        batch.is_inline_entity = true;
        batch.face_cull_plane = [0.0; 4];
        batch.face_cull_runs = Arc::from([]);
        batch.inline_model_slot = inline_slot_for_batch[index];
        // The fourth cull-record draw word identifies inline movers. Bits
        // may change on snapshots, but their original geometry is immutable.
        if let Some(record) = cull_records.get_mut(batch.cull_index as usize) {
            record.draw[3] = 1;
            record.face_plane = [0.0; 4];
        }
    }
    for (index, batch) in full_batches[inline_full_start..].iter_mut().enumerate() {
        batch.is_inline_entity = true;
        batch.face_cull_plane = [0.0; 4];
        batch.face_cull_runs = Arc::from([]);
        batch.inline_model_slot = inline_slot_for_batch[index];
        if let Some(record) = cull_records.get_mut(batch.cull_index as usize) {
            record.draw[3] = 1;
            record.face_plane = [0.0; 4];
        }
    }
    let mut inline_group_states = Vec::<DrawBatch>::new();
    for batch in coarse_batches[inline_coarse_start..]
        .iter_mut()
        .chain(full_batches[inline_full_start..].iter_mut())
    {
        let group = inline_group_states
            .iter()
            .position(|state| same_draw_state(state, &batch.source))
            .unwrap_or_else(|| {
                inline_group_states.push(batch.source.clone());
                inline_group_states.len() - 1
            });
        batch.inline_group = u32::try_from(group).unwrap_or(u32::MAX);
    }
    let inline_group_count = inline_group_states.len();
    let inline_indirect_capacity = u32::try_from(
        (coarse_batches.len() - inline_coarse_start)
            .max(full_batches.len() - inline_full_start)
            .max(1),
    )
    .unwrap_or(u32::MAX);
    // Inline source ranges index `inline_vertices`, not `vertices`. Move them
    // past every world range so world-indexed debug paths (surface inspector
    // picking/highlight) skip them instead of aliasing a world surface.
    let world_vertex_count = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    for batch in coarse_batches[inline_coarse_start..]
        .iter_mut()
        .chain(full_batches[inline_full_start..].iter_mut())
    {
        let range = &mut batch.source.vertices;
        *range = range.start.saturating_add(world_vertex_count)
            ..range.end.saturating_add(world_vertex_count);
    }
    let inline_models = inline_model_sources
        .iter()
        .zip(inline_gpu_starts)
        .zip(inline_rt_indices)
        .zip(inline_rt_masks)
        .map(
            |(((model, gpu_vertex_start), rt_indices), rt_mask_sources)| InlineModelGpu {
                model: model.model,
                gpu_vertex_start,
                base: inline_vertices[model.vertices.start as usize..model.vertices.end as usize]
                    .to_vec(),
                rt_indices,
                raster_glass_local: crate::renderer::lighting::glass_transmission::raster_glass_triangles(&rt_mask_sources),
                rt_mask_sources,
                coarse_batches: model
                    .batches
                    .clone()
                    .map(|index| inline_coarse_start + index)
                    .collect(),
                full_batches: if visibility.is_some() {
                    model
                        .batches
                        .clone()
                        .map(|index| inline_full_start + index)
                        .collect()
                } else {
                    Vec::new()
                },
                uploaded: None,
            },
        )
        .collect::<Vec<_>>();
    upload_timings.batch_bind_groups_ms = batch_started.elapsed().as_secs_f64() * 1000.0;
    let compact_groups =
        build_draw_compaction_groups(&mut cull_records, &mut coarse_batches, &mut full_batches);
    // Recipe batches are deliberately not compacted: a warm plan draws each
    // recipe as one real indexed draw. Their FULL fallback pieces keep FULL's
    // compaction groups.
    let mut auto4 = if visibility.is_some() {
        instantiate_portal_draw_plans(
            &portal_draw_plan,
            &full_batches,
            inline_full_start,
            &mut cull_records,
            &mut initial_indirect,
        )
    } else {
        None
    };
    bsp_model_lod.bind(&coarse_batches, &full_batches);
    patch_lod.bind(&mut coarse_batches,0);
    patch_lod.bind(&mut full_batches,1);
    if let Some(instance) = &mut auto4 {
        patch_lod.bind(&mut instance.batches[..instance.variant_base],2);
        patch_lod.recipes(&instance.geometry_members, &full_batches, &mut instance.geometry_states);
        bsp_model_lod.recipes(&instance.geometry_members, &full_batches, &mut instance.geometry_states);
        for (geometry, state) in instance.geometry_states.iter().enumerate() {
            if matches!(state, Auto4GeometryState::Uncached) {
                for &variant in &instance.geometry_variants[geometry] {
                    let batch = &mut instance.batches[instance.variant_base+variant];
                    batch.patch_geometry = None;
                    batch.indexed_range = 0..0;
                    cull_records[batch.cull_index as usize].draw[0] = 0;
                    initial_indirect[batch.cull_index as usize].index_count = 0;
                }
            }
        }
    }
    let compact_draw_capacity = compact_groups
        .iter()
        .map(|group| group.max_count)
        .sum::<u32>();
    if !compact_groups.is_empty() {
        rverbose!(
            2,
            "GPU draw compaction: {} compatible material group(s), {} candidate draws",
            compact_groups.len(),
            compact_draw_capacity
        );
    }
    if !portal_draw_plan.plan_by_cluster.is_empty() {
        rverbose!(
            2,
            "AUTO 4 plans: {} FULL piece(s), {} recipe(s) over {} geometr(ies) ({} drawn from existing indices), {} unique cluster plan(s) for {} cluster(s); {} recipe reuse hit(s), {} whole-plan reuse hit(s); full eager collapse would cost {:.2} MiB, lazy cache budget {:.2} MiB",
            portal_draw_plan.piece_count,
            portal_draw_plan.variants.len(),
            portal_draw_plan.geometries.len(),
            auto4.as_ref().map_or(0, |auto4| auto4
                .geometry_states
                .iter()
                .filter(|state| matches!(state, Auto4GeometryState::Static))
                .count()),
            portal_draw_plan.plans.len(),
            portal_draw_plan.plan_by_cluster.len(),
            portal_draw_plan.reused_variant_hits,
            portal_draw_plan.reused_plan_hits,
            portal_draw_plan.packed_index_count as f64 * std::mem::size_of::<u32>() as f64
                / (1024.0 * 1024.0),
            auto4_cache_bytes as f64 / (1024.0 * 1024.0),
        );
    }
    // Carry map-authored shader-path and pipeline-class provenance in existing
    // cull-record flag bits. q3map_forceMeta bakes original models into BSP
    // surfaces; models/ is a useful DIAGNOSTIC HEURISTIC, not original entity ID.
    // AUTO4 recipes use their representative material and receive the same bits.
    for batch in coarse_batches.iter()
        .chain(full_batches.iter())
        .chain(auto4.as_ref().into_iter().flat_map(|instance| instance.batches.iter()))
    {
        let model_path = !batch.is_inline_entity && material_debug.entries
            .get(batch.source.material_debug_index as usize)
            .is_some_and(|entry| entry.name.starts_with("models/"));
        if let Some(record) = cull_records.get_mut(batch.cull_index as usize) {
            if model_path { record.draw[3] |= 4; }
            if batch.source.pipeline.class == DrawClass::Transparent { record.draw[3] |= 8; }
            record.draw[3] |= world_blend_probe_bucket(batch.source.pipeline) << 5;
            // Bit 8 identifies authored water. Keep independent from material
            // name, so an ocean with textures/ or models/ qpaths still works.
            record.draw[3] |= u32::from(batch.source.water) << 8;
        }
    }
    let cull_started = Instant::now();
    let cull_count = u32::try_from(cull_records.len()).unwrap_or(u32::MAX);
    if cull_records.is_empty() {
        cull_records.push(GpuCullRecord::zeroed());
        initial_indirect.push(DrawIndexedIndirectArgs {
            index_count: 0,
            instance_count: 1,
            first_index: 0,
            base_vertex: 0,
            first_instance: 0,
        });
    }
    let cull_records_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA GPU cull records"),
        contents: bytemuck::cast_slice(&cull_records),
        // COPY_DST: inline-model (mover) bounds are rewritten as they move.
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let indirect_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA indirect draw arguments"),
        contents: bytemuck::cast_slice(&initial_indirect),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_DST,
    });
    let early_indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("JKA Hi-Z early indirect draw arguments"),
        size: (cull_records.len().max(1) * std::mem::size_of::<DrawIndexedIndirectArgs>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
        mapped_at_creation: false,
    });
    let active_cull_indices = vec![0_u32; usize::try_from(cull_count.max(1)).unwrap_or(1)];
    let active_cull_indices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA active GPU cull indices"),
        contents: bytemuck::cast_slice(&active_cull_indices),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let compact_indirect = vec![
        DrawIndexedIndirectArgs::zeroed();
        usize::try_from(compact_draw_capacity.max(1)).unwrap_or(1)
    ];
    let compact_indirect_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA compacted indirect draw arguments"),
        contents: bytemuck::cast_slice(&compact_indirect),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_DST,
    });
    let reflection_compact_capacity = compact_draw_capacity
        .saturating_mul(PLANAR_REFLECTION_SLOTS as u32)
        .max(1);
    let reflection_compact_indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("JKA planar reflection compacted indirect draw arguments"),
        size: u64::from(reflection_compact_capacity)
            * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
        usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let inline_indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("JKA mover multi-draw arguments"),
        size: u64::from(inline_indirect_capacity)
            * (PLANAR_REFLECTION_SLOTS as u64 + 1)
            * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
        usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let compact_counts = vec![0_u32; compact_groups.len().max(1)];
    let compact_count_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA compacted indirect draw counts"),
        contents: bytemuck::cast_slice(&compact_counts),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::COPY_DST,
    });
    let cull_debug_reasons = vec![0_u32; usize::try_from(cull_count.max(1)).unwrap_or(1)];
    let cull_debug_reason_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA cull rejection reasons"),
        contents: bytemuck::cast_slice(&cull_debug_reasons),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let cull_debug_count_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA cull rejection counters"),
        contents: bytemuck::cast_slice(&[0_u32; 4]),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
    });
    let cull_debug_bind_group =
        create_cull_debug_bind_group(device, cull_debug_layout, &cull_debug_reason_buffer);
    upload_timings.cull_resources_ms = cull_started.elapsed().as_secs_f64() * 1000.0;
    let lighting_started = Instant::now();
    if dynamic_lights.len() > MAX_DYNAMIC_LIGHTS {
        eprintln!(
            "Dynamic lights: {} source lights found; clustered enhancement uses the first {}",
            dynamic_lights.len(),
            MAX_DYNAMIC_LIGHTS
        );
    }
    let dynamic_lights = dynamic_lights
        .into_iter()
        .take(MAX_DYNAMIC_LIGHTS)
        .collect::<Vec<_>>();
    let light_count = u32::try_from(dynamic_lights.len()).unwrap_or(0);
    let shadow_slot_by_light = vec![0_u32; dynamic_lights.len()];
    // Static map lights occupy the prefix; transient CGame/FX lights use the
    // remaining slots at runtime without reallocating this storage buffer.
    let mut gpu_lights = gpu_point_lights(&dynamic_lights, &shadow_slot_by_light);
    gpu_lights.resize(MAX_DYNAMIC_LIGHTS, GpuPointLight::zeroed());
    let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA dynamic point lights"),
        contents: bytemuck::cast_slice(&gpu_lights),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let local_shadows = create_local_shadow_resources(device, shadow_caster_layout);
    let sunlight_glass_count = crate::renderer::lighting::glass_transmission::upload_raster_sun_glass(
        queue, &local_shadows.glass_buffer, &raster_glass_static, &[]);
    if sunlight_glass_count > crate::renderer::lighting::glass_transmission::RASTER_SUN_GLASS_SLOTS
        * crate::renderer::lighting::glass_transmission::RASTER_GLASS_PER_LIGHT {
        eprintln!("[JKA TINTED GLASS] {} sun triangles exceed raster capacity: sunlight tint disabled for this map", sunlight_glass_count);
    }

    let static_light_grid_gpu = upload_static_light_grid(device, queue, static_light_grid.as_ref());
    let entity_light_grid = static_light_grid
        .map(scene::StaticLightGrid::into_classic_entity_grid)
        .map(|mut grid| {
            if let Some(sun) = sun {
                grid.estimate_sun_weights(sun.direction, sun.color);
            }
            grid
        });
    let voxel_probe_gi = upload_voxel_probe_gi(device, queue, voxel_probe_gi_data.as_ref());
    if light_count != 0 {
        rverbose!(
            1,
            "Local light shadow cache: {} active slot(s), {} cached 512x512 cubemap slot(s); lights selected by camera/PVS relevance",
            MAX_LOCAL_SHADOW_LIGHTS,
            LOCAL_SHADOW_CACHE_SLOTS
        );
    }
    let cluster_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("JKA clustered-light lists"),
        size: u64::from(CLUSTER_COUNT)
            * u64::from(1 + MAX_CLUSTER_LIGHTS)
            * std::mem::size_of::<u32>() as u64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let legacy_triangle_surfaces = if legacy_dlight_triangle_surfaces.is_empty() {
        vec![u32::MAX]
    } else {
        legacy_dlight_triangle_surfaces.clone()
    };
    let surface_count = legacy_dlight_surfaces.len().max(1);
    // Keep the writable Legacy masks first. Immutable per-surface fits follow
    // them, so existing partial mask uploads cannot overwrite the calibration.
    let mut initial_legacy_masks = vec![0u32; surface_count * 5];
    if let Some(fit) = baked_sun_calibration {
        for (surface, data) in fit.surfaces.iter().enumerate() {
            for (component, value) in data.iter().enumerate() {
                initial_legacy_masks[surface_count + surface * 4 + component] = value.to_bits();
            }
        }
    }
    let legacy_dlight_surface_mask_buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA Legacy dlight surface masks"),
            contents: bytemuck::cast_slice(&initial_legacy_masks),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

    let cluster_compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA clustered-light compute bind group"),
        layout: cluster_compute_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: cluster_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: lighting_settings_buffer.as_entire_binding(),
            },
        ],
    });
    let lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA clustered-light render bind group"),
        layout: lighting_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: cluster_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: lighting_settings_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pbr_settings_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&local_shadows.cube_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&local_shadows.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&static_light_grid_gpu.direction_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&static_light_grid_gpu.lighting_view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::Sampler(&static_light_grid_gpu.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: static_light_grid_gpu.uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&voxel_probe_gi.point_view),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(&voxel_probe_gi.area_view),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: voxel_probe_gi.uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: wgpu::BindingResource::TextureView(
                    &static_light_grid_gpu.irradiance_volume_view,
                ),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: static_light_grid_gpu
                    .irradiance_volume_uniform_buffer
                    .as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: legacy_dlight_surface_mask_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 17,
                resource: local_shadows.glass_buffer.as_entire_binding(),
            },
        ],
    });

    let lighting_bind_group_lean = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA clustered-light render bind group (lean baseline)"),
        layout: lighting_layout_lean,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: cluster_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: lighting_settings_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pbr_settings_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&local_shadows.cube_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&local_shadows.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&static_light_grid_gpu.direction_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&static_light_grid_gpu.lighting_view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::Sampler(&static_light_grid_gpu.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: static_light_grid_gpu.uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: legacy_dlight_surface_mask_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 17,
                resource: local_shadows.glass_buffer.as_entire_binding(),
            },
        ],
    });
    upload_timings.lighting_resources_ms = lighting_started.elapsed().as_secs_f64() * 1000.0;

    let pipeline_started = Instant::now();
    // Pointer-style PSO reuse on the remap mutation path: material stage
    // descriptors still change, but a compatible compiled PSO never recompiles.
    let reusable_pipeline = reusable_world
        .filter(|old| old.remap_scene_format == surface_format
            && old.remap_msaa_samples == msaa_samples)
        .and_then(|old| old.pipeline_variants.get(&shader_variant).map(|psos| (old, psos)))
        .filter(|(_, psos)| {
            let keys = coarse_batches.iter().map(|b| b.source.pipeline).collect::<BTreeSet<_>>();
            keys.iter().all(|key| psos.pipelines.contains_key(key))
                && (!shader_variant.legacy_dlights || coarse_batches.iter()
                    .filter(|b| crate::renderer::legacy_dlight_receives(&b.source))
                    .all(|b| psos.legacy_dlight_pipelines.contains_key(&b.source.pipeline)))
                && (!legacy_fog || crate::renderer::legacy_fog_pass_jobs(&coarse_batches)
                    .iter().all(|job| psos.fog_pass_pipelines.contains_key(job)))
                && (planar_reflectors.is_empty() || !shader_variant.planar_reflections
                    || keys.iter().all(|key| psos.reflection_pipelines.contains_key(key)))
                && (planar_reflectors.is_empty() || !shader_variant.planar_reflections || !legacy_fog
                    || crate::renderer::legacy_fog_pass_jobs(&coarse_batches)
                        .iter().all(|job| psos.reflection_fog_pass_pipelines.contains_key(job)))
                && (!shader_variant.material_specialization ||
                    material_specialization_jobs(coarse_batches.iter().chain(&full_batches))
                        .iter().all(|job| psos.specialized_pipelines.contains_key(job)))
        });
    let (pipeline_variants, fast_pipelines) = if let Some((old, compiled)) = reusable_pipeline {
        rverbose!(1, "[SHADER REMAP] reused existing world PSOs (no pipeline compilation)");
        let mut variants = BTreeMap::new();
        variants.insert(shader_variant, compiled.clone());
        let fast = if coarse_batches.iter().all(|b| old.fast_pipelines.contains_key(&b.source.pipeline)) {
            old.fast_pipelines.clone()
        } else {
            BTreeMap::new()
        };
        (variants, fast)
    } else {
        let pipelines = create_world_pipelines(
            device,
            pipeline_layout,
            shader,
            surface_format,
            msaa_samples,
            &coarse_batches,
            legacy_fog,
            Some(shader_variant),
        );
        let legacy_dlight_pipelines = if shader_variant.legacy_dlights {
            create_legacy_dlight_pass_pipelines(
                device,
                pipeline_layout,
                shader,
                surface_format,
                msaa_samples,
                &coarse_batches,
                shader_variant,
            )
        } else {
            BTreeMap::new()
        };
        let fog_pass_pipelines = if legacy_fog {
            create_legacy_fog_pass_pipelines(
                device,
                pipeline_layout,
                shader,
                surface_format,
                msaa_samples,
                &coarse_batches,
                Some(shader_variant),
            )
        } else {
            BTreeMap::new()
        };
        // The known-fast baseline pipelines are compiled on first use by
        // `ensure_fast_world_pipelines`; most sessions never leave the advanced path.
        let fast_pipelines = BTreeMap::new();
        let reflection_pipelines = if planar_reflectors.is_empty() || !shader_variant.planar_reflections
        {
            BTreeMap::new()
        } else {
            create_reflection_pipelines(
                device,
                pipeline_layout,
                shader,
                surface_format,
                &coarse_batches,
                legacy_fog,
                shader_variant,
            )
        };
        let reflection_fog_pass_pipelines =
            if legacy_fog && !planar_reflectors.is_empty() && shader_variant.planar_reflections {
                create_reflection_fog_pass_pipelines(
                    device,
                    pipeline_layout,
                    shader,
                    surface_format,
                    &coarse_batches,
                    shader_variant,
                )
            } else {
                BTreeMap::new()
            };
        if let Some(path) = std::env::var_os("JKA_DUMP_WORLD_BATCHES") {
            dump_world_batches(&path, &coarse_batches, &full_batches, shader_variant, legacy_fog,
                &material_debug);
        }
        let specialized_pipelines = create_specialized_world_pipelines(
            device,
            pipeline_layout,
            shader,
            surface_format,
            msaa_samples,
            &material_specialization_jobs(coarse_batches.iter().chain(&full_batches)),
            shader_variant,
        );
        let mut pipeline_variants = BTreeMap::new();
        pipeline_variants.insert(
            shader_variant,
            WorldPipelineVariant {
                ocean_far_pipeline: None,
                pipelines,
                legacy_dlight_pipelines,
                fog_pass_pipelines,
                reflection_pipelines,
                reflection_fog_pass_pipelines,
                specialized_pipelines,
            },
        );
        (pipeline_variants, fast_pipelines)
    };
    upload_timings.pipeline_create_ms = pipeline_started.elapsed().as_secs_f64() * 1000.0;

    let visibility_started = Instant::now();
    let coarse_all_batches: Vec<usize> = (0..coarse_batches.len()).collect();
    // Only a map with a sky portal needs the FULL no-PVS fallback list.
    // Keep its allocation out of ordinary maps and the hot frame path.
    let full_all_batches: Vec<usize> = if sky_portal.is_some() {
        (0..full_batches.len()).collect()
    } else {
        Vec::new()
    };
    let coarse_visible_batches_by_cluster = visibility
        .as_ref()
        .map(|vis| visible_batches_by_cluster(&coarse_batches, vis))
        .unwrap_or_default();
    let full_visible_batches_by_cluster = visibility
        .as_ref()
        .filter(|_| !full_batches.is_empty())
        .map(|vis| visible_batches_by_cluster(&full_batches, vis))
        .unwrap_or_default();
    if let (Some(auto4), false) = (&auto4, coarse_visible_batches_by_cluster.is_empty()) {
        let plan_stats =
            |batches: &[WorldBatch], plans: &[Vec<usize>], by_cluster: Option<&[usize]>| {
                let mut total_batches = 0usize;
                let mut total_triangles = 0u64;
                let mut max_batches = 0usize;
                let mut max_triangles = 0u64;
                let cluster_count = by_cluster.map_or(plans.len(), |map| map.len()).max(1);
                for cluster in 0..cluster_count {
                    let plan_id = by_cluster
                        .and_then(|map| map.get(cluster).copied())
                        .unwrap_or(cluster);
                    let Some(indices) = plans.get(plan_id) else {
                        continue;
                    };
                    let triangles = indices
                        .iter()
                        .filter_map(|&index| batches.get(index))
                        .map(|batch| {
                            u64::from(
                                batch
                                    .indexed_range
                                    .end
                                    .saturating_sub(batch.indexed_range.start)
                                    / 3,
                            )
                        })
                        .sum::<u64>();
                    total_batches = total_batches.saturating_add(indices.len());
                    total_triangles = total_triangles.saturating_add(triangles);
                    max_batches = max_batches.max(indices.len());
                    max_triangles = max_triangles.max(triangles);
                }
                (
                    total_batches as f64 / cluster_count as f64,
                    total_triangles as f64 / cluster_count as f64,
                    max_batches,
                    max_triangles,
                )
            };
        let (minimal_avg_batches, minimal_avg_tris, minimal_max_batches, minimal_max_tris) =
            plan_stats(&coarse_batches, &coarse_visible_batches_by_cluster, None);
        let piece_tris = |index: usize| {
            full_batches.get(index).map_or(0, |batch| {
                u64::from(
                    batch
                        .indexed_range
                        .end
                        .saturating_sub(batch.indexed_range.start)
                        / 3,
                )
            })
        };
        let always_batches = auto4.variant_base.saturating_sub(auto4.always_start);
        let always_tris = (auto4.always_start..auto4.variant_base)
            .map(piece_tris)
            .sum::<u64>();
        let mut auto4_total_batches = 0usize;
        let mut auto4_total_tris = 0u64;
        let mut auto4_max_batches = 0usize;
        let mut auto4_max_tris = 0u64;
        let auto4_cluster_count = portal_draw_plan.plan_by_cluster.len().max(1);
        for &plan_id in &portal_draw_plan.plan_by_cluster {
            let Some(refs) = portal_draw_plan.plans.get(plan_id) else {
                continue;
            };
            let mut triangles = always_tris;
            for reference in refs {
                triangles += match *reference {
                    PreparedPortalPlanBatchRef::Base(index) => piece_tris(index),
                    PreparedPortalPlanBatchRef::Merged(variant) => auto4.variant_members[variant]
                        .iter()
                        .map(|&member| piece_tris(member))
                        .sum(),
                };
            }
            let batch_count = refs.len().saturating_add(always_batches);
            auto4_total_batches = auto4_total_batches.saturating_add(batch_count);
            auto4_total_tris = auto4_total_tris.saturating_add(triangles);
            auto4_max_batches = auto4_max_batches.max(batch_count);
            auto4_max_tris = auto4_max_tris.max(triangles);
        }
        let auto4_avg_batches = auto4_total_batches as f64 / auto4_cluster_count as f64;
        let auto4_avg_tris = auto4_total_tris as f64 / auto4_cluster_count as f64;
        rverbose!(
            2,
            "PVS batch pressure: MINIMAL avg {:.1} batch(es) / {:.0} tris, max {} / {} tris; AUTO 4 avg {:.1} batch(es) / {:.0} tris, max {} / {} tris",
            minimal_avg_batches,
            minimal_avg_tris,
            minimal_max_batches,
            minimal_max_tris,
            auto4_avg_batches,
            auto4_avg_tris,
            auto4_max_batches,
            auto4_max_tris,
        );
    }
    upload_timings.visibility_tables_ms = visibility_started.elapsed().as_secs_f64() * 1000.0;

    let Auto4Instance {
        batches: auto4_batches,
        variant_base: auto4_variant_base,
        always_start: auto4_always_start,
        variant_members: auto4_variant_members,
        variant_geometry: auto4_variant_geometry,
        variant_area_mixed: auto4_variant_area_mixed,
        geometry_members: auto4_geometry_members,
        geometry_variants: auto4_geometry_variants,
        geometry_states: auto4_geometry_states,
    } = auto4.unwrap_or(Auto4Instance {
        batches: Vec::new(),
        variant_base: 0,
        always_start: 0,
        variant_members: Vec::new(),
        variant_geometry: Vec::new(),
        variant_area_mixed: Vec::new(),
        geometry_members: Vec::new(),
        geometry_variants: Vec::new(),
        geometry_states: Vec::new(),
    });
    let auto4_plan_refs = if auto4_batches.is_empty() {
        Vec::new()
    } else {
        portal_draw_plan.plans.clone()
    };
    let auto4_plan_by_cluster = portal_draw_plan.plan_by_cluster.clone();
    let lazy_geometries = auto4_geometry_states
        .iter()
        .filter(|state| matches!(state, Auto4GeometryState::Uncached))
        .count();
    let mut auto4_collapse_cache = Auto4CollapseCache::new(auto4_geometry_states);
    auto4_collapse_cache.capacity_indices = auto4_cache_indices;
    if auto4_cache_indices > 0 && lazy_geometries > 0 {
        let cache_end = base_index_count.saturating_add(auto4_cache_indices);
        auto4_collapse_cache
            .free_ranges
            .push(base_index_count..cache_end);
        match Auto4CollapseWorker::spawn(
            Arc::clone(&auto4_source_indices),
            Arc::clone(&auto4_piece_ranges),
        ) {
            Ok(worker) => {
                auto4_collapse_cache.worker = Some(worker);
                rverbose!(
                    2,
                    "AUTO 4 lazy physical-collapse cache: {:.2} MiB, {} geometr(ies) build on demand with FULL-piece fallback",
                    auto4_cache_bytes as f64 / (1024.0 * 1024.0),
                    lazy_geometries,
                );
            }
            Err(error) => {
                rverbose!(
                    1,
                    "AUTO 4 lazy physical-collapse worker unavailable ({error}); non-contiguous recipes draw their FULL pieces"
                );
            }
        }
    }

    let grass_started = Instant::now();
    let grass = grass_renderer.upload_map(device, queue, &grass_patches, &grass_local_fogs);
    upload_timings.grass_upload_ms = grass_started.elapsed().as_secs_f64() * 1000.0;

    let static_ao_source = static_ao_cache_source.map(|cache| StaticAoWorldSource {
        cache,
        vertices: Arc::new(vertices),
        vertex_ao_receivers: Arc::new(static_ao_vertex_receivers),
        lightmap_triangles: Arc::new(static_ao_lightmap_triangles),
        render_triangles: Arc::new(static_ao_render_triangles),
        lightmap_sizes: Arc::new(static_ao_lightmap_sizes),
        lightmap_bases: Arc::new(static_ao_lightmap_bases),
    });

    let ocean_sky_representatives = ocean_sky_regions.iter().enumerate()
        .map(|(slot, extension)| extension.and_then(|_| full_batches.iter().position(|batch|
            batch.ocean_clipmap.is_some() && batch.ocean_clipmap_id as usize == slot
                && batch.source.water_primary)))
        .collect::<Vec<_>>();

    (
        WorldGpu {
            remap_index_source: Arc::clone(&auto4_source_indices),
            remap_asset_source,
            remap_asset_hash,
            remap_texture_hashes,
            remap_base_texture_count,
            remap_texture_usage: texture_usage,
            remap_picmip: picmip,
            remap_compression: texture_compression,
            remap_pbr_compression: compress_pbr_companions,
            remap_scene_format: surface_format,
            remap_msaa_samples: msaa_samples,
            has_glass_primary: coarse_batches
                .iter()
                .chain(&full_batches)
                .any(|batch| batch.source.glass_primary),
            animated_batches: None,
            patch_lod,
            bsp_model_lod,
            mark_surfaces,
            source_map,
            source_map_lighting,
            sky_average,
            sky_portal,
            primary_skybox,
            sky_covers_background,
            ocean_surfaces,
            ocean_meshes_built: build_ocean_meshes,
            ocean_promotion_decisions,
            promoted_oceans: upload_timings.promoted_oceans.clone(),
            ocean_masks: Arc::new(ocean_masks.clone()),
            ocean_base_masks: Arc::new(ocean_masks),
            ocean_sky_regions,
            ocean_sky_representatives,
            ocean_pvs_footprints,
            runtime_ocean_clipmaps,
            inline_models,
            vertex_buffer,
            index_buffer,
            rt_shadow_ranges,
            rt_shadow_masks,
            glass_texture_names,
            raster_glass_static,
            raster_glass_dynamic: Vec::new(),
            rt_vertex_count,
            ocean_coarsest_spacing: upload_timings.ocean_coarsest_spacing,
            cull_records_buffer,
            indirect_buffer,
            early_indirect_buffer,
            active_cull_indices_buffer,
            compact_indirect_buffer,
            compact_count_buffer,
            reflection_compact_indirect_buffer,
            reflection_compact_capacity,
            inline_indirect_buffer,
            inline_indirect_capacity,
            inline_group_count,
            cull_debug_reason_buffer,
            cull_debug_count_buffer,
            cull_debug_bind_group,
            cull_record_count: cull_records.len(),
            cull_bind_group: None,
            active_cull_count: 0,
            active_selection_key: None,
            compact_groups,
            light_buffer,
            _cluster_buffer: cluster_buffer,
            legacy_dlight_surface_id_buffer,
            legacy_dlight_surface_mask_buffer,
            legacy_dlight: crate::renderer::lighting::legacy_dlight_lists::LegacyDlightLists::new(
                legacy_dlight_surfaces.len(),
            ),
            legacy_dlight_triangle_surfaces: legacy_triangle_surfaces,
            legacy_dlight_surfaces,
            dynamic_lights,
            cluster_compute_bind_group,
            lighting_bind_group,
            lighting_bind_group_lean,
            _static_light_grid: static_light_grid_gpu,
            entity_light_grid,
            voxel_probe_gi,
            reflection_probes,
            froxel_bind_group: None,
            light_count,
            local_shadows,
            sun,
            baked_sun_radiance,
            baked_sun_encoding: baked_sun_encoding.filter(|_| authored_sun_projected
                && lightmap_data.iter().all(|image| image.rgba16f.is_none() && image.mip_level_count == 1)),
            baked_sun_cache: None,
            grass,
            surface_sprite_effects,
            surface_sprite_resident,
            active_pipeline_variant: shader_variant,
            pipeline_variants,
            fast_pipelines,
            fast_complexity_pipelines: BTreeMap::new(),
            complexity_pipelines: BTreeMap::new(),
            planar_reflectors,
            coarse_batches,
            full_batches,
            visibility,
            coarse_visible_batches_by_cluster,
            full_visible_batches_by_cluster,
            auto4_batches,
            auto4_plan_refs,
            auto4_plan_by_cluster,
            auto4_variant_members,
            auto4_variant_geometry,
            auto4_variant_area_mixed,
            auto4_geometry_members,
            auto4_geometry_variants,
            auto4_variant_base,
            auto4_always_start,
            auto4_active_plan: Vec::new(),
            // FULL aliases keep FULL's indices, so inline movers map 1:1.
            auto4_inline_start: inline_full_start,
            auto4_inline_full_start: inline_full_start,
            auto4_collapse_cache,
            coarse_all_batches,
            full_all_batches,
            last_cluster: None,
            ocean_gate_cache: None,
            textures,
            footprint_mark_textures,
            lightmaps,
            deluxemaps,
            // BC workers have joined by this point; recover the original Vec
            // without copying multi-megabyte lightmap pixels in the common case.
            lightmap_base_data: Arc::try_unwrap(lightmap_data)
                .unwrap_or_else(|data| data.as_ref().clone()),
            static_ao_source,
            static_ao_source_to_gpu,
            static_ao_gpu_vertices,
            static_ao_lightmap_active: false,
            texture_clamp,
            detail_texture_by_base,
            material_debug,
            inspector_vertices,
            inspector_textures,
            inspector_lightmaps,
            snow_shell,
        },
        upload_timings,
    )
}
