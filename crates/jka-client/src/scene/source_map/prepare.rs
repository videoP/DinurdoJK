//! Source map prepare.
use crate::scene::{
    add_bounds, append_sky_batches, assign_planar_reflection_planes, authored_sun,
    build_voxel_probe_gi, cache_reflection_decisions, can_fold_jka_lightmap_pair,
    canonical_map_shader, class_for, fs, is_utility_shader, load_movement, log_reflection_cache,
    map_asset_name, map_chunk_key, map_dynamic_lights, map_portal_surface_anchors,
    map_spawn_points, map_world_lighting, map_worldspawn_distance_cull, mark_surfaces_from_batches,
    material_debug_info, material_uv_size, materials, planar_group_key, prepare,
    prepare_with_jobs_options, prepared_stages, push_face_triangles, reconstruct_source_brushes,
    render_position, retain_authored_planar_mirrors, shader_is_render_utility,
    source_collision_brush, stage_batch, Arc, AssetSearchPath, BTreeMap, BTreeSet, DVec3,
    DrawClass, Instant, MapDocument, MapFileStats, MapGeometry, MapGroupKey, MapJobPool,
    MapLoadTimings, MapMaterial, MapPrepareOptions, MapSource, Path, PrepStageKeys, PreparedMap,
    PreparedPortalDrawPlan, SpawnPoint, TcGen, Textures, MAX_MAP_FILE_BYTES,
};

pub fn prepare_source(
    root: &Path,
    game: Option<&Path>,
    source: &MapSource,
) -> Result<PreparedMap, String> {
    match source {
        MapSource::Bsp(name) => prepare(root, game, name),
        MapSource::Map(name) => prepare_map_asset(root, game, name),
        MapSource::MapFile(path) => prepare_map_file(root, game, path),
        MapSource::MapEditPreview { path, text } => {
            prepare_map_edit_preview(root, game, path, text, None, MapPrepareOptions::default())
        }
    }
}

pub fn prepare_source_with_jobs(
    root: &Path,
    game: Option<&Path>,
    source: &MapSource,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
    seed: Option<&Arc<PreparedMap>>,
) -> Result<PreparedMap, String> {
    match source {
        MapSource::Bsp(name) => prepare_with_jobs_options(root, game, name, jobs, options, seed),
        MapSource::Map(name) => prepare_map_asset_with_jobs(root, game, name, jobs, options),
        MapSource::MapFile(path) => prepare_map_file_with_jobs(root, game, path, jobs, options),
        MapSource::MapEditPreview { path, text } => {
            prepare_map_edit_preview(root, game, path, text, Some(jobs), options)
        }
    }
}

pub fn prepare_map_asset(
    root: &Path,
    game: Option<&Path>,
    name: &str,
) -> Result<PreparedMap, String> {
    prepare_map_asset_inner(root, game, name, None, MapPrepareOptions::default())
}

pub(in crate::scene) fn prepare_map_asset_with_jobs(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    prepare_map_asset_inner(root, game, name, Some(jobs), options)
}

pub(in crate::scene) fn prepare_map_asset_inner(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    let asset_name = map_asset_name(name, "map")?;
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    let asset = assets
        .read(&asset_name, MAX_MAP_FILE_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!("Source map {asset_name} not found on the game/base asset search path")
        })?;
    let text = std::str::from_utf8(&asset.bytes)
        .map_err(|error| format!("{asset_name} is not UTF-8 text: {error}"))?;
    let document =
        jka_assets::map::parse(text).map_err(|error| format!("{asset_name}: {error}"))?;
    prepare_map_document_with_assets_options(&asset.source, document, &mut assets, jobs, options)
}

pub fn prepare_map_file(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
) -> Result<PreparedMap, String> {
    prepare_map_file_inner(root, game, path, None, MapPrepareOptions::default())
}

pub(in crate::scene) fn prepare_map_file_with_jobs(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    prepare_map_file_inner(root, game, path, Some(jobs), options)
}

pub(in crate::scene) fn prepare_map_file_inner(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("map"))
    {
        return Err(format!(
            "Loose map source must use a .map extension: {}",
            path.display()
        ));
    }
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not stat loose map {}: {error}", path.display()))?;
    if metadata.len() > MAX_MAP_FILE_BYTES as u64 {
        return Err(format!(
            "Loose map {} exceeds {} MiB limit",
            path.display(),
            MAX_MAP_FILE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read loose map {}: {error}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("Loose map {} is not UTF-8 text: {error}", path.display()))?;
    let document =
        jka_assets::map::parse(text).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    prepare_map_document_with_assets_options(path, document, &mut assets, jobs, options)
}

pub(in crate::scene) fn prepare_map_edit_preview(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
    text: &str,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    if !path
        .extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("map"))
    {
        return Err(format!(
            "Edited source-map preview must refer to a .map file: {}",
            path.display()
        ));
    }
    if text.len() > MAX_MAP_FILE_BYTES {
        return Err(format!(
            "Edited map preview {} exceeds {} MiB limit",
            path.display(),
            MAX_MAP_FILE_BYTES / (1024 * 1024)
        ));
    }
    let document = jka_assets::map::parse(text)
        .map_err(|error| format!("Edited preview {}: {error}", path.display()))?;
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    prepare_map_document_with_assets_options(path, document, &mut assets, jobs, options)
}

#[cfg(test)]
pub(in crate::scene) fn prepare_map_document(
    root: &Path,
    game: Option<&Path>,
    path: &Path,
    document: MapDocument,
) -> Result<PreparedMap, String> {
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    prepare_map_document_with_assets_options(
        path,
        document,
        &mut assets,
        None,
        MapPrepareOptions::default(),
    )
}

pub(in crate::scene) fn prepare_map_document_with_assets_options(
    path: &Path,
    document: MapDocument,
    assets: &mut AssetSearchPath,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    assets.set_allow_asset_overrides(options.allow_asset_overrides);
    let document = Arc::new(document);
    let world_indices: Vec<_> = document
        .entities
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| (entity.classname() == Some("worldspawn")).then_some(index))
        .collect();
    let Some(&world_index) = world_indices.first() else {
        return Err("Source .map has no worldspawn entity".into());
    };

    let mut warnings = Vec::new();
    if world_indices.len() > 1 {
        warnings.push(format!(
            "{} worldspawn entities found; rendering only the first",
            world_indices.len()
        ));
    }
    if document.stats.patches_skipped != 0 {
        warnings.push(format!(
            "{} patchDef2/patchDef3 object(s) skipped (Milestone 1 supports brushes only)",
            document.stats.patches_skipped
        ));
    }
    if document.stats.unsupported_objects_skipped != 0 {
        warnings.push(format!(
            "{} unsupported map object block(s) skipped",
            document.stats.unsupported_objects_skipped
        ));
    }
    if document.stats.degenerate_faces != 0 {
        warnings.push(format!(
            "{} degenerate brush face plane(s) ignored while parsing",
            document.stats.degenerate_faces
        ));
    }

    // q3map2 treats func_group as an editor/compiler grouping construct rather
    // than a runtime brush entity. Its brushes belong to the static world.
    // Keep true runtime/submodel brush entities separate until entity/model
    // transforms and behavior are implemented.
    let grouped_world_indices: Vec<_> = document
        .entities
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| (entity.classname() == Some("func_group")).then_some(index))
        .collect();
    let grouped_world_brushes: usize = grouped_world_indices
        .iter()
        .map(|index| document.entities[*index].brushes.len())
        .sum();

    let skipped_entity_brushes = document
        .entities
        .iter()
        .enumerate()
        .filter(|(index, entity)| {
            *index != world_index
                && entity.classname() != Some("worldspawn")
                && entity.classname() != Some("func_group")
        })
        .map(|(_, entity)| entity.brushes.len())
        .sum();
    if skipped_entity_brushes != 0 {
        warnings.push(format!(
            "{skipped_entity_brushes} runtime/non-static entity brush(es) parsed but skipped"
        ));
    }
    let additional_world_brushes: usize = world_indices
        .iter()
        .skip(1)
        .map(|index| document.entities[*index].brushes.len())
        .sum();
    if additional_world_brushes != 0 {
        warnings.push(format!(
            "{additional_world_brushes} brush(es) in additional worldspawn entities skipped"
        ));
    }

    let world = &document.entities[world_index];
    let distance_cull = map_worldspawn_distance_cull(world, &mut warnings);
    let mut static_entity_indices = Vec::with_capacity(1 + grouped_world_indices.len());
    static_entity_indices.push(world_index);
    static_entity_indices.extend(grouped_world_indices.iter().copied());
    let (library, library_debug) = if let Some(jobs) = jobs {
        materials::shader_library_with_jobs(assets, &mut warnings, jobs, options.pbr_materials)?
    } else {
        materials::shader_library(assets, &mut warnings, options.pbr_materials)?
    };
    let mut textures = Textures::new();
    let mut material_lookup = BTreeMap::<String, usize>::new();
    let mut map_materials = Vec::<MapMaterial>::new();
    let mut groups = BTreeMap::<MapGroupKey, MapGeometry>::new();
    let mut triangles = 0usize;
    let mut rendered_faces = 0usize;
    let mut utility_faces_skipped = 0usize;
    let mut skipped_brushes = 0usize;
    let mut bounds = None;
    let mut transparent_order = 0usize;

    let reconstruction_started = Instant::now();
    let reconstructed_brushes =
        reconstruct_source_brushes(Arc::clone(&document), &static_entity_indices, jobs)?;
    let reconstruction_ms = reconstruction_started.elapsed().as_secs_f64() * 1000.0;
    let mut source_collision_brushes = Vec::new();

    for item in reconstructed_brushes {
        let entity_index = item.entity_index;
        let brush_index = item.brush_index;
        let entity = &document.entities[entity_index];
        let entity_kind = if entity_index == world_index {
            "worldspawn"
        } else {
            "func_group"
        };
        let brush = &entity.brushes[brush_index];
        let reconstructed = match item.result {
            Ok(brush) => brush,
            Err(error) => {
                skipped_brushes += 1;
                warnings.push(format!(
                    "{entity_kind} entity {entity_index} brush {brush_index} (line {}): {error}; brush skipped",
                    brush.line
                ));
                continue;
            }
        };
        for &point in &reconstructed.vertices {
            add_bounds(&mut bounds, point);
        }

        // Gameplay collision is deliberately generated before the render-only
        // utility-surface filter below. Caulk/nodraw/playerclip must stay in CM
        // even though they never consume a WGPU draw batch.
        if let Some(collision_brush) = source_collision_brush(brush, &reconstructed, &library) {
            source_collision_brushes.push(collision_brush);
        }

        // Match the conservative Radiant large-map index: one static brush
        // belongs to the 1024-unit cell containing its centre. A brush may
        // extend beyond that cell; renderer batch bounds are computed from
        // the actual vertices, so frustum rejection remains conservative.
        let brush_chunk = map_chunk_key(&reconstructed.vertices);

        for polygon in reconstructed.faces {
            let face = &brush.faces[polygon.face_index];
            if is_utility_shader(&face.shader) {
                utility_faces_skipped += 1;
                continue;
            }

            let shader_name = canonical_map_shader(&face.shader, &library);
            if library
                .get(&shader_name)
                .is_some_and(shader_is_render_utility)
            {
                utility_faces_skipped += 1;
                continue;
            }
            let material_index = if let Some(&index) = material_lookup.get(&shader_name) {
                index
            } else {
                let material = materials::describe(
                    &shader_name,
                    0,
                    library.get(&shader_name),
                    library_debug.origins.get(&shader_name),
                    assets,
                    &mut textures,
                    options.gen_normal_maps,
                    options.omit_environment_stages,
                );
                let uv_size = material_uv_size(&material, &textures);
                let index = map_materials.len();
                map_materials.push(MapMaterial { material, uv_size });
                material_lookup.insert(shader_name, index);
                index
            };
            let map_material = &map_materials[material_index];
            if map_material.material.hidden {
                continue;
            }

            // Build this face first so environment-mapped opaque geometry can
            // use the same coplanar grouping rule as the BSP path. The old
            // source-map path isolated every tcGen environment face, which can
            // turn a large map into thousands of WGPU commands.
            let mut face_vertices =
                Vec::with_capacity(polygon.vertices.len().saturating_sub(2).saturating_mul(3));
            let face_triangles = push_face_triangles(
                &mut face_vertices,
                face,
                &polygon.vertices,
                map_material.uv_size,
            );
            if face_triangles == 0 {
                continue;
            }

            let class = class_for(&map_material.material);
            let has_environment_stage = map_material
                .material
                .stages
                .iter()
                .any(|stage| matches!(stage.tc_gen, TcGen::Environment));
            let planar_candidate = options.planar_reflections
                && class != DrawClass::Transparent
                && (map_material.material.planar_reflection
                    || (options.planar_environment && has_environment_stage));
            let planar_group = planar_candidate
                .then(|| planar_group_key(&face_vertices))
                .flatten();
            let preserve_unique_plane = planar_candidate && planar_group.is_none();
            if class == DrawClass::Transparent || preserve_unique_plane {
                transparent_order += 1;
            }
            let unique_submission = class == DrawClass::Transparent || preserve_unique_plane;
            let spatial_submission =
                planar_group.is_some() || (options.source_spatial_batches && !unique_submission);
            let key = MapGroupKey {
                material: material_index,
                transparent_order: if unique_submission {
                    transparent_order
                } else {
                    0
                },
                chunk: spatial_submission.then_some(brush_chunk),
                planar_group,
            };
            groups
                .entry(key)
                .or_default()
                .vertices
                .extend(face_vertices);
            triangles += face_triangles;
            rendered_faces += 1;
        }
    }

    let sun = authored_sun(
        material_lookup.keys().map(String::as_str),
        &library,
        &mut warnings,
    );

    let material_debug = material_debug_info(
        &library_debug,
        material_lookup.iter().filter_map(|(name, &index)| {
            Some((name.clone(), map_materials.get(index)?.material.clone()))
        }),
        &library,
        &textures,
    );

    let Some((minimum, maximum)) = bounds else {
        return Err("No bounded static world brushes could be reconstructed".into());
    };

    let spatial_chunks = groups
        .keys()
        .filter_map(|key| key.chunk)
        .collect::<BTreeSet<_>>()
        .len();
    let geometry_groups = groups.len();
    let collision_brush_count = source_collision_brushes.len();

    let mut vertices = Vec::new();
    let mut batches = Vec::new();
    for (key, geometry) in groups {
        if geometry.vertices.is_empty() {
            continue;
        }
        let start = u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;
        vertices.extend_from_slice(&geometry.vertices);
        let end = u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;
        let range = start..end;
        let material = &map_materials[key.material].material;

        if material.sky {
            append_sky_batches(
                &mut batches,
                material,
                None,
                None,
                false,
                range,
                None,
                &[],
                [0_u64; 4],
                [0.0; 4],
                false,
            );
            continue;
        }

        let stages = prepared_stages(material, false);
        if stages.is_empty() {
            continue;
        }
        if !material.explicit {
            batches.push(stage_batch(
                material,
                &stages[0],
                true,
                false,
                range,
                None,
                None,
                None,
                &[],
                [0_u64; 4],
                [0.0; 4],
                false,
            ));
        } else if stages.len() >= 2 && can_fold_jka_lightmap_pair(&stages[0], &stages[1]) {
            // A direct .map has no compiled `$lightmap` texture. Rendering the
            // authored Q3/JKA `$lightmap` + GL_DST_COLOR/GL_ZERO pair literally
            // therefore leaves no sensible receiver for the realtime q3map2
            // preview (and can collapse to white/black depending on fallback
            // bindings). Treat the diffuse half as the opaque source surface.
            // With simulation off this is the expected fullbright editor look;
            // with simulation on the normal opaque receiver is multiplied by the
            // q3map2-equivalent direct/ambient lighting. Decorative stages after
            // the canonical pair retain their authored order.
            let mut diffuse = stages[1].clone();
            diffuse.blend = None;
            diffuse.depth_write = true;
            diffuse.depth_equal = false;
            let mut folded = stage_batch(
                material,
                &diffuse,
                true,
                false,
                range.clone(),
                None,
                None,
                None,
                &[],
                [0_u64; 4],
                [0.0; 4],
                false,
            );
            // The `$lightmap` stage is folded away, so this batch is the receiver.
            folded.dlight_in_lightmap_stage = false;
            batches.push(folded);
            for stage in stages.iter().skip(2) {
                batches.push(stage_batch(
                    material,
                    stage,
                    false,
                    false,
                    range.clone(),
                    None,
                    None,
                    None,
                    &[],
                    [0_u64; 4],
                    [0.0; 4],
                    false,
                ));
            }
        } else {
            for (stage_index, stage) in stages.iter().enumerate() {
                batches.push(stage_batch(
                    material,
                    stage,
                    stage_index == 0,
                    false,
                    range.clone(),
                    None,
                    None,
                    None,
                    &[],
                    [0_u64; 4],
                    [0.0; 4],
                    false,
                ));
            }
        }
    }

    let (portal_anchors, camera_portal_count) = map_portal_surface_anchors(&document);
    let authored_portal_batch_count;
    if options.planar_reflections {
        assign_planar_reflection_planes(&mut batches, &vertices, options.planar_environment);
        authored_portal_batch_count = batches
            .iter()
            .filter(|batch| batch.planar_reflection)
            .count();
        retain_authored_planar_mirrors(&mut batches, &portal_anchors);
    } else {
        authored_portal_batch_count = 0;
        for batch in &mut batches {
            batch.planar_reflection = false;
            batch.planar_environment_candidate = false;
            batch.planar_plane = [0.0; 4];
        }
    }
    cache_reflection_decisions(&mut batches);
    let draw_batches = batches.len();
    log_reflection_cache(&path.display().to_string(), &batches);
    let planar_mirror_batch_count = batches
        .iter()
        .filter(|batch| batch.planar_reflection)
        .count();
    if authored_portal_batch_count != 0 && planar_mirror_batch_count == 0 {
        warnings.push(
            "portal shader surfaces found, but none has an untargeted misc_portal_surface within 64 units; no planar mirrors enabled"
                .into(),
        );
    }
    if options.planar_reflections && camera_portal_count != 0 {
        warnings.push(format!(
            "{camera_portal_count} targeted misc_portal_surface camera portal(s) detected; planar mirror pass leaves camera portals on their authored material"
        ));
    }

    warnings.extend(textures.warnings);
    warnings.sort();
    warnings.dedup();

    let center = (minimum + maximum) * 0.5;
    let movement = load_movement(assets, &mut warnings);
    let collision =
        match jka_movement::CollisionWorld::from_source_brushes(source_collision_brushes) {
            Ok(world) => Some(world),
            Err(error) => {
                warnings.push(format!("Source-map player collision unavailable: {error}"));
                None
            }
        };
    let mut spawns = map_spawn_points(&document);
    if spawns.is_empty() {
        let spawn_jka = DVec3::new(center.x, center.y, maximum.z + 64.0);
        spawns.push(SpawnPoint {
            position: render_position(spawn_jka.to_array().map(|value| value as f32)),
            yaw: 0.0,
            ..SpawnPoint::default()
        });
    }

    let lights = map_dynamic_lights(&document);
    let source_map_lighting = map_world_lighting(world);
    if source_map_lighting.ambient != [0.0; 3] || source_map_lighting.minlight != [0.0; 3] {
        devprintln!(
            2,
            "Source .map q3map2 baseline: ambient={:.3},{:.3},{:.3} minlight={:.3},{:.3},{:.3}",
            source_map_lighting.ambient[0],
            source_map_lighting.ambient[1],
            source_map_lighting.ambient[2],
            source_map_lighting.minlight[0],
            source_map_lighting.minlight[1],
            source_map_lighting.minlight[2],
        );
    }
    let voxel_probe_gi = options
        .voxel_probe_gi
        .then(|| build_voxel_probe_gi(&vertices, &batches, &lights, sun))
        .flatten();

    let mark_surfaces = mark_surfaces_from_batches(&vertices, &batches);
    Ok(PreparedMap {
        authored_oceans: Vec::new(),
        vertices,
        legacy_dlight_triangle_surfaces: Vec::new(),
        legacy_dlight_surfaces: Vec::new(),
        batches,
        pvs_batches: Vec::new(),
        portal_draw_plan: PreparedPortalDrawPlan::default(),
        debug_volumes: Arc::default(),
        inline_vertices: Vec::new(),
        inline_batches: Vec::new(),
        inline_models: Vec::new(),
        movement,
        collision,
        mark_surfaces: Some(mark_surfaces),
        static_models: Arc::default(),
        physics_collision: crate::cgame::ragdoll::PhysicsMapMesh::default(),
        weather_occlusion: None,
        videos: textures.videos,
        textures: textures.images,
        footprint_mark_textures: [None; 2],
        footprint_mark_blend_modes: [0; 2],
        lightmaps: Vec::new(),
        deluxemaps: Vec::new(),
        static_bsp_ao_cache: None,
        steam_audio_acoustic_mesh: None,
        steam_audio_bake: None,
        steam_audio_bake_request: None,
        visibility: None,
        lights,
        source_map_lighting,
        sun,
        static_light_grid: None,
        voxel_probe_gi,
        reflection_probes: Vec::new(),
        grass_patches: Vec::new(),
        grass_local_fogs: vec![[0.0; 4]],
        surface_sprite_effects: Vec::new(),
        global_fog: None,
        warnings,
        spawns,
        fx_runners: Vec::new(),
        brush_entities: Vec::new(),
        entity_graph: None,
        distance_cull,
        sky_portal: None,
        triangles,
        lightmap_pages: 0,
        source: path.to_path_buf(),
        material_debug,
        map_hash: 0,
        stage_keys: PrepStageKeys::default(),
        texture_hashes: Vec::new(),
        bsp_stats: None,
        load_timings: MapLoadTimings {
            geometry_ms: reconstruction_ms,
            worker_count: jobs.map_or(1, MapJobPool::worker_count),
            ..MapLoadTimings::default()
        },
        map_file_stats: Some(MapFileStats {
            entities: document.stats.entities,
            brushes: document.stats.brushes,
            world_brushes: world.brushes.len(),
            grouped_world_brushes,
            rendered_faces,
            utility_faces_skipped,
            skipped_entity_brushes,
            patches_skipped: document.stats.patches_skipped,
            degenerate_faces: document.stats.degenerate_faces,
            skipped_brushes,
            spatial_chunks,
            geometry_groups,
            draw_batches,
            collision_brushes: collision_brush_count,
            spatial_batching: options.source_spatial_batches,
            worker_count: jobs.map_or(1, MapJobPool::worker_count),
            reconstruction_ms,
        }),
    })
}
