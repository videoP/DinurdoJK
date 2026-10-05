//! World upload.
use crate::renderer::{
    average_skybox_color, build_draw_compaction_groups, collect_planar_reflectors,
    collect_static_ao_lightmap_triangles, collect_static_ao_render_triangles,
    collect_static_ao_vertex_receivers, create_cull_debug_bind_group, create_fast_world_bind_group,
    create_legacy_dlight_pass_pipelines, create_legacy_fog_pass_pipelines,
    create_local_shadow_resources, create_reflection_fog_pass_pipelines,
    create_reflection_pipelines, create_snow_shell_gpu, create_world_bind_group,
    create_world_pipelines, detail_texture_for_source, gpu_point_lights, inspector_texture_meta,
    instantiate_portal_draw_plans, is_upward_water_face, legacy_dlight_runs_for_batch,
    make_cull_record, material_uniform, ocean_surface_key, rt_alpha_mask_source_from_gpu_vertices,
    same_draw_state, scene, upload_reflection_probes, upload_static_light_grid, upload_texture,
    upload_texture_bc3_picmip, upload_texture_picmip, upload_voxel_probe_gi,
    visible_batches_by_cluster, water_surface_extent, weather, Arc, Auto4CollapseCache,
    Auto4CollapseWorker, Auto4GeometryState, Auto4Instance, BTreeMap, BTreeSet, BlendMode,
    DrawBatch, DrawClass, DrawIndexedIndirectArgs, GpuCullRecord, GpuImage, GpuPointLight,
    GpuVertex, GrassRenderer, HashMap, IndexedGeometryBuilder, InlineModelGpu, InspectorVertex,
    Instant, MaterialUniform, Mutex, PreparedMap, PreparedPortalPlanBatchRef, RtAlphaMaskSource,
    StaticAoBaseLightmap, StaticAoWorldSource, SurfaceSpriteEffectGpu, SurfaceSpriteEffectRenderer,
    TextureData, UniformArena, WorldBatch, WorldGpu, WorldPipelineVariant, WorldShaderVariantKey,
    WorldUploadTimings, AUTO4_COLLAPSE_CACHE_BYTES, AUTO_DETAIL_ENABLED_BIT, CLUSTER_COUNT,
    LOCAL_SHADOW_CACHE_SLOTS, MAX_CLUSTER_LIGHTS, MAX_DYNAMIC_LIGHTS, MAX_LOCAL_SHADOW_LIGHTS,
    MAX_OCEAN_SURFACES, PLANAR_REFLECTION_SLOTS,
};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

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
    map: PreparedMap,
) -> (WorldGpu, WorldUploadTimings) {
    let source_map = map.map_file_stats.is_some();
    let sky_portal = map.sky_portal;
    let PreparedMap {
        vertices,
        legacy_dlight_triangle_surfaces,
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
        sun,
        static_light_grid,
        voxel_probe_gi: voxel_probe_gi_data,
        reflection_probes: reflection_probe_data,
        grass_patches,
        grass_local_fogs,
        surface_sprite_effects: surface_sprite_effect_sources,
        material_debug,
        inline_vertices,
        inline_batches: inline_batch_sources,
        inline_models: inline_model_sources,
        mark_surfaces,
        ..
    } = map;
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
    upload_timings.prelude_ms = prelude_started.elapsed().as_secs_f64() * 1000.0;

    let vertex_started = Instant::now();
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
    let mut ocean_clipmaps = BTreeMap::<[u32; 5], ([std::ops::Range<u32>; 2], u8)>::new();
    let mut ocean_surfaces = Vec::<crate::ocean::OceanSurface>::new();
    let mut ocean_masks = crate::ocean::OceanMasks::default();
    let mut ocean_coarsest_spacing = 0.0f32;
    if build_ocean_meshes {
        let mut water_faces = Vec::<crate::ocean::WaterFace>::new();
        let mut water_face_keys = Vec::<[u32; 5]>::new();
        let mut water_face_triangles = Vec::<Vec<crate::ocean::MaskTriangle>>::new();
        let mut seen_faces = BTreeSet::<[u32; 5]>::new();
        for source in coarse_sources.iter().chain(full_sources.iter()) {
            if !source.water_primary || !is_upward_water_face(source, &vertices) {
                continue;
            }
            let (plane, minimum, maximum) = water_surface_extent(source, &vertices);
            let key = ocean_surface_key(plane, minimum, maximum);
            if !seen_faces.insert(key) {
                continue;
            }
            let slice = &vertices[source.vertices.start as usize..source.vertices.end as usize];
            water_face_triangles.push(
                slice
                    .chunks_exact(3)
                    .map(|t| [0, 1, 2].map(|i| [t[i].position[0], t[i].position[2]]))
                    .collect(),
            );
            water_face_keys.push(key);
            water_faces.push(crate::ocean::WaterFace {
                plane,
                minimum,
                maximum,
                authored_ocean: source.authored_ocean,
            });
        }
        let clusters = crate::ocean::cluster_water_faces(&water_faces);
        if clusters.len() > MAX_OCEAN_SURFACES {
            rverbose!(
                1,
                "Ocean: {} promoted water surfaces from {} faces; the {} smallest stay on their authored faces",
                clusters.len(),
                water_faces.len(),
                clusters.len() - MAX_OCEAN_SURFACES,
            );
        }
        for (slot, cluster) in clusters.iter().take(MAX_OCEAN_SURFACES).enumerate() {
            let authored = water_faces[cluster.members[0]].authored_ocean.is_some();
            // A map-authored ocean carries its own bounds, so it takes no mask.
            let mask_slot = if authored {
                None
            } else {
                let triangles: Vec<crate::ocean::MaskTriangle> = cluster
                    .members
                    .iter()
                    .flat_map(|&member| water_face_triangles[member].iter().copied())
                    .collect();
                ocean_masks.push_surface(slot, &triangles).then_some(slot)
            };
            // Both mesh qualities are uploaded so the quality setting stays a
            // live switch instead of needing a map reload.
            let low = crate::ocean::build_clipmap(
                crate::ocean::OCEAN_LOW_MESH_SPACING_UNITS,
                cluster.plane,
                cluster.minimum,
                cluster.maximum,
                mask_slot,
            );
            let high = crate::ocean::build_clipmap(
                crate::ocean::OCEAN_HIGH_MESH_SPACING_UNITS,
                cluster.plane,
                cluster.minimum,
                cluster.maximum,
                mask_slot,
            );
            ocean_coarsest_spacing = low.coarsest_spacing;
            upload_timings.ocean_clipmap_vertices +=
                (low.vertices.len() + high.vertices.len()) as u64;
            upload_timings.ocean_clipmap_tris += (high.indices.len() / 3) as u64;
            upload_timings.ocean_clipmap_spacing = crate::ocean::OCEAN_HIGH_MESH_SPACING_UNITS;
            upload_timings.ocean_coarsest_spacing = low.coarsest_spacing;
            let ranges = [
                indexed_builder.append_clipmap(&low.vertices, &low.indices),
                indexed_builder.append_clipmap(&high.vertices, &high.indices),
            ];
            for &member in &cluster.members {
                ocean_clipmaps.insert(water_face_keys[member], (ranges.clone(), slot as u8));
            }
            ocean_surfaces.push(crate::ocean::OceanSurface {
                plane_height: cluster.plane,
                minimum: cluster.minimum,
                maximum: cluster.maximum,
            });
        }
    }
    let _ = ocean_coarsest_spacing;
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
    let auto4_source_indices = Arc::new(std::mem::take(&mut indexed_builder.indices));
    let base_index_count = u32::try_from(auto4_source_indices.len()).unwrap_or(u32::MAX);
    let base_index_bytes =
        (auto4_source_indices.len() as u64).saturating_mul(std::mem::size_of::<u32>() as u64);
    // Never reserve more than building every non-contiguous recipe would need.
    let requested_auto4_cache_bytes = (portal_draw_plan.packed_index_count as u64)
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
            bytemuck::cast_slice(auto4_source_indices.as_ref()),
        );
    }
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
    let texture_started = Instant::now();
    let texture_clamp: Vec<_> = texture_data.iter().map(|data| data.clamp).collect();
    // Companion maps are only safe to BC-compress when that image is not also
    // referenced as authored color/emissive/sky data. The optional benchmark
    // path uses BC3 because it preserves all RGB channels plus alpha (required
    // by normalHeightMap and rmosMap) without changing shader decoding.
    const TEX_USE_COMPANION: u8 = 1;
    const TEX_USE_COLOR: u8 = 2;
    let mut texture_usage = vec![0u8; texture_data.len()];
    let mut mark = |index: Option<usize>, usage: u8| {
        if let Some(index) = index {
            if let Some(bits) = texture_usage.get_mut(index) {
                *bits |= usage;
            }
        }
    };
    let mut skybox_faces: Vec<usize> = Vec::new();
    for source in coarse_sources.iter().chain(full_sources.iter()) {
        mark(source.texture, TEX_USE_COLOR);
        mark(source.emissive_texture, TEX_USE_COLOR);
        mark(source.normal_texture, TEX_USE_COMPANION);
        mark(source.roughness_texture, TEX_USE_COMPANION);
        mark(source.height_texture, TEX_USE_COMPANION);
        mark(source.metallic_texture, TEX_USE_COMPANION);
        mark(source.specular_texture, TEX_USE_COMPANION);
        if let Some(skybox) = source.skybox {
            for index in skybox {
                mark(Some(index), TEX_USE_COLOR);
                skybox_faces.push(index);
            }
        }
    }
    let primary_skybox = coarse_sources
        .iter()
        .chain(full_sources.iter())
        .find_map(|source| source.skybox);
    let sky_average = average_skybox_color(&texture_data, &skybox_faces);
    for index in footprint_mark_textures.into_iter().flatten() {
        mark(Some(index), TEX_USE_COLOR);
    }
    for effect in &surface_sprite_effect_sources {
        mark(Some(effect.texture), TEX_USE_COLOR);
    }
    let mut compressed_count = 0usize;
    let textures: Vec<_> = texture_data
        .iter()
        .enumerate()
        .map(|(index, data)| {
            let use_bc3 = compress_pbr_companions
                && !data.srgb
                && texture_usage.get(index).copied().unwrap_or_default() == TEX_USE_COMPANION;
            // TaystJK r_smartpicmip defaults on and disables picmip for any
            // image whose qpath is not under textures/. Keep that compatibility
            // rule rather than degrading videoMap/gfx/UI-style dependencies that
            // merely happen to be referenced by a world shader.
            let texture_picmip = if data
                .label
                .get(..9)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("textures/"))
            {
                picmip
            } else {
                0
            };
            if use_bc3 {
                compressed_count += 1;
                upload_texture_bc3_picmip(device, queue, data, texture_picmip)
            } else {
                upload_texture_picmip(device, queue, data, texture_picmip)
            }
        })
        .collect();
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
    upload_timings.texture_upload_ms = texture_started.elapsed().as_secs_f64() * 1000.0;
    let lightmap_started = Instant::now();
    let lightmaps: Vec<_> = lightmap_data
        .iter()
        .map(|data| upload_texture(device, queue, data))
        .collect();
    let deluxemaps: Vec<_> = if deluxemap_data.is_empty() {
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
                    // too. Those are not the water surface, so they neither
                    // become ocean nor draw on their own.
                    source.water_primary = false;
                }
            }
            let (ocean_clipmap, ocean_clipmap_id) =
                match water_plane.and_then(|(plane, minimum, maximum)| {
                    ocean_clipmaps
                        .get(&ocean_surface_key(plane, minimum, maximum))
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
            let material_uniform =
                material_uniform(&source, (detail_auto_word & AUTO_DETAIL_ENABLED_BIT) != 0);
            let material_buffer =
                material_arena.push(device, queue, bytemuck::bytes_of(&material_uniform));
            let fog_uniform = weather::fog::surface_fog_uniform(
                source.fog,
                source.fog_is_global,
                source.fog_color_override,
                source.legacy2_fog_in_stage_safe,
                source.global_fog_post_eligible,
            );
            let fog_buffer = fog_arena.push(device, queue, bytemuck::bytes_of(&fog_uniform));
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
                &fog_buffer,
                surface_deformation_buffer,
                deformation_field_views,
                deformation_field_sampler,
            );
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
                surface_deformation_buffer,
                deformation_field_views,
                deformation_field_sampler,
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
            cull_records.push(record);
            let legacy_dlight_runs =
                legacy_dlight_runs_for_batch(&source, &index_range, triangle_surfaces);
            uploaded.push(WorldBatch {
                indexed_range: index_range,
                legacy_dlight_runs,
                is_inline_entity: false,
                inline_group: u32::MAX,
                ocean_clipmap,
                ocean_clipmap_id,
                source,
                bind_group,
                fast_bind_group,
                _material_buffer: material_buffer,
                _fog_buffer: fog_buffer,
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
    let planar_reflectors = collect_planar_reflectors(&coarse_batches, &vertices);
    // Inline models join both batch sets after the static world, so every
    // static batch index (reflectors, snow, AO) is unchanged. Their empty PVS
    // signature keeps them in every cluster's list.
    let inline_coarse_start = coarse_batches.len();
    let mut rt_shadow_ranges = Vec::<std::ops::Range<u32>>::new();
    let mut rt_shadow_surfaces = BTreeSet::<(u32, u32)>::new();
    for batch in &coarse_batches[..inline_coarse_start] {
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
        if batch.source.pipeline.class != DrawClass::Mask
            || batch.source.pipeline.blend != BlendMode::Opaque
            || !batch.source.pipeline.depth_write
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
        if let Some(mask) =
            rt_alpha_mask_source_from_gpu_vertices(&vertices, &batch.source, texture)
        {
            rt_shadow_masks.push(mask);
        }
    }
    let rt_vertex_count = u32::try_from(indexed_builder.vertices.len()).unwrap_or(u32::MAX);
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
                    batch.pipeline.class == DrawClass::Opaque
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
                if batch.pipeline.class != DrawClass::Mask
                    || batch.pipeline.blend != BlendMode::Opaque
                    || !batch.pipeline.depth_write
                    || batch.vertices.is_empty()
                {
                    continue;
                }
                let surface_key = (batch.vertices.start, batch.vertices.end);
                if opaque_surfaces.contains(&surface_key) || !seen_masks.insert(surface_key) {
                    continue;
                }
                let texture = batch.texture.and_then(|index| texture_data.get(index));
                if let Some(mask) =
                    rt_alpha_mask_source_from_gpu_vertices(&inline_vertices, batch, texture)
                {
                    masks.push(mask);
                }
            }
            masks
        })
        .collect::<Vec<_>>();
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
    for batch in &mut coarse_batches[inline_coarse_start..] {
        batch.is_inline_entity = true;
    }
    for batch in &mut full_batches[inline_full_start..] {
        batch.is_inline_entity = true;
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
    let auto4 = if visibility.is_some() {
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
    let initial_legacy_masks = vec![0u32; legacy_dlight_surfaces.len().max(1)];
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
        ],
    });
    upload_timings.lighting_resources_ms = lighting_started.elapsed().as_secs_f64() * 1000.0;

    let pipeline_started = Instant::now();
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
    let mut pipeline_variants = BTreeMap::new();
    pipeline_variants.insert(
        shader_variant,
        WorldPipelineVariant {
            pipelines,
            legacy_dlight_pipelines,
            fog_pass_pipelines,
            reflection_pipelines,
            reflection_fog_pass_pipelines,
        },
    );
    upload_timings.pipeline_create_ms = pipeline_started.elapsed().as_secs_f64() * 1000.0;

    let visibility_started = Instant::now();
    let coarse_all_batches: Vec<usize> = (0..coarse_batches.len()).collect();
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

    (
        WorldGpu {
            mark_surfaces,
            source_map,
            source_map_lighting,
            sky_average,
            sky_portal,
            primary_skybox,
            ocean_surfaces,
            ocean_masks: Arc::new(ocean_masks),
            inline_models,
            vertex_buffer,
            index_buffer,
            rt_shadow_ranges,
            rt_shadow_masks,
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
            legacy_dlight_surface_masks_cpu: Mutex::new(initial_legacy_masks),
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
            grass,
            surface_sprite_effects,
            active_pipeline_variant: shader_variant,
            pipeline_variants,
            fast_pipelines,
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
            last_cluster: None,
            textures,
            footprint_mark_textures,
            lightmaps,
            deluxemaps,
            lightmap_base_data: lightmap_data,
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
