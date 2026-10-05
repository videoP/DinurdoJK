//! Prepare.
use crate::scene::{
    append_authored_ocean_planes, append_material_batches, assign_planar_reflection_planes,
    assign_reflection_probes, authored_sun, bsp_brush_entities, bsp_dynamic_lights, bsp_fx_runners,
    bsp_global_fog_num, bsp_global_fog_params, bsp_physics_collision_mesh,
    bsp_portal_surface_anchors, bsp_sky_portal, bsp_surface_lights, bsp_weather_occlusion_source,
    bsp_worldspawn_distance_cull, build_prepared_portal_draw_plan, build_voxel_probe_gi,
    cache_reflection_decisions, class_for, collect_grass_emitters,
    collect_surface_sprite_effect_emitters, embedded_deluxe_mapping, embedded_deluxemap_texture,
    embedded_lightmap_texture, external_lightmap_pages, finish_grass_patches_with_jobs,
    generate_grass_chunk, grass_fingerprint, grass_ground_albedo_tint, grass_lightmap_sources,
    grass_local_fog_slots, hdr_lightmap_indexed, load_external_lightmap,
    load_legacy_footprint_marks, load_movement, load_reflection_probes, log_reflection_cache,
    map_asset_name, map_content_hash, material_debug_info, material_render_is_guaranteed_discarded,
    materials, neutral_deluxemap, order_pvs_pieces, phase_lap, planar_group_key,
    prepare_static_light_grid, pvs_signature, render_position, resolved_bsp_surface_fog,
    retain_authored_planar_mirrors, static_bsp_ao_cache_info, try_load_external_deluxemap,
    try_load_hdr_lightmap, Arc, AssetSearchPath, Atlas, BTreeMap, BTreeSet, Bsp, BspMapStats,
    DrawClass, Geometry, GpuVertex, GrassInstance, GrassPatchKey, GroupKey, InlineModelGeometry,
    Instant, LegacyDlightSurface, MapJobPool, MapLoadTimings, MapPrepareOptions, Path,
    PrepStageKeys, PreparedMap, PreparedPortalDrawPlan, SourceMapLighting, SpawnPoint,
    StageFingerprint, SurfaceKind, SurfaceMaterial, Task, TcGen, TextureData, Textures, Vec3,
    WorldGeometryChunk, LIGHTMAP_BY_VERTEX, MAX_FILE_BYTES,
};

pub fn prepare(root: &Path, game: Option<&Path>, name: &str) -> Result<PreparedMap, String> {
    prepare_internal(root, game, name, None, MapPrepareOptions::default(), None)
}

pub fn prepare_with_options(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    options: MapPrepareOptions,
) -> Result<PreparedMap, String> {
    prepare_internal(root, game, name, None, options, None)
}

/// `prepare_with_options` that reuses unchanged stages of an earlier preparation.
#[cfg(test)]
pub fn prepare_with_seed(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    options: MapPrepareOptions,
    seed: &Arc<PreparedMap>,
) -> Result<PreparedMap, String> {
    prepare_internal(root, game, name, None, options, Some(seed))
}

pub fn prepare_with_jobs_options(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
    seed: Option<&Arc<PreparedMap>>,
) -> Result<PreparedMap, String> {
    prepare_internal(root, game, name, Some(jobs), options, seed)
}

/// Records a stage lent by the seed and shows it as finished on the loading panel.
pub(in crate::scene) fn note_reused_stage(
    jobs: Option<&MapJobPool>,
    reused: &mut Vec<&'static str>,
    label: &'static str,
    task: Task,
) {
    reused.push(label);
    if let Some(jobs) = jobs {
        jobs.mark_reused(task);
    }
}

pub(in crate::scene) fn prepare_internal(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    jobs: Option<&MapJobPool>,
    options: MapPrepareOptions,
    seed: Option<&Arc<PreparedMap>>,
) -> Result<PreparedMap, String> {
    let prepare_started = Instant::now();
    let mut load_timings = MapLoadTimings {
        worker_count: jobs.map_or(1, MapJobPool::worker_count),
        ..Default::default()
    };
    let asset_name = map_asset_name(name, "bsp")?;
    let archive_opens_before = jka_assets::pk3::archive_open_stats();

    let stage = Instant::now();
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    assets.set_allow_asset_overrides(options.allow_asset_overrides);
    load_timings.asset_index_ms = stage.elapsed().as_secs_f64() * 1000.0;

    let stage = Instant::now();
    let asset = assets
        .read(&asset_name, MAX_FILE_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Map {asset_name} not found on the game/base asset search path"))?;
    load_timings.bsp_read_ms = stage.elapsed().as_secs_f64() * 1000.0;
    let asset_source = asset.source.clone();
    let bytes = Arc::new(asset.bytes);
    // FNV-1a over the whole BSP is a serial byte chain (tens of ms on a big map),
    // and its value names on-disk caches so it cannot change. With a worker pool
    // and no seed to validate, it runs beside the BSP parse on an idle worker and
    // is joined once the parse is done; otherwise it is computed right here.
    let hash_job = match jobs {
        Some(jobs) if seed.is_none() => {
            let hash_bytes = Arc::clone(&bytes);
            Some(jobs.submit(Task::MapPrepare, move || {
                map_content_hash(hash_bytes.as_slice())
            })?)
        }
        _ => None,
    };
    let inline_hash = hash_job
        .is_none()
        .then(|| map_content_hash(bytes.as_slice()));

    let mut warnings = Vec::new();
    // A previous preparation of this same map (the app's restart cache) lends
    // every stage whose inputs are unchanged, so a forced re-prepare does not
    // redo them. Nothing extra is stored: the seed is data the app already holds.
    let seed = seed.filter(|seed| inline_hash == Some(seed.map_hash));
    let mut reused = Vec::<&'static str>::new();
    // Collision and the acoustic mesh read only the BSP bytes.
    let seed_collision = seed.and_then(|seed| seed.collision.clone());
    let seed_acoustic = seed.and_then(|seed| seed.steam_audio_acoustic_mesh.clone());
    if seed_collision.is_some() {
        note_reused_stage(jobs, &mut reused, "collision", Task::MapCollision);
    }

    let mut steam_audio_job = None;
    let mut steam_audio_sync = None;
    // The collision world is not needed until after texture preload has been
    // queued, so with a worker pool it is joined later (see below) instead of
    // stalling the loader right after the BSP parse.
    let mut collision_pending = None;
    let (bsp, mesh, collision) = if let Some(jobs) = jobs {
        let parse_bytes = Arc::clone(&bytes);
        let parse = jobs.submit(Task::MapBspParse, move || {
            let started = Instant::now();
            let result = Bsp::parse(parse_bytes.as_slice())
                .map_err(|error| error.to_string())
                .and_then(|bsp| {
                    let mesh = bsp.world_mesh(4).map_err(|error| error.to_string())?;
                    Ok((Arc::new(bsp), Arc::new(mesh)))
                });
            (result, started.elapsed().as_secs_f64() * 1000.0)
        })?;

        if seed_collision.is_none() {
            let collision_bytes = Arc::clone(&bytes);
            collision_pending = Some(jobs.submit(Task::MapCollision, move || {
                let started = Instant::now();
                let result = jka_movement::CollisionWorld::from_bsp(collision_bytes.as_slice());
                (result, started.elapsed().as_secs_f64() * 1000.0)
            })?);
        }

        // While BSP/collision CPU work is running, keep the loader thread useful:
        // animation.cfg IO and shader-file reads overlap those worker jobs.
        let movement_stage = Instant::now();
        let movement = load_movement(&mut assets, &mut warnings);
        load_timings.movement_ms = movement_stage.elapsed().as_secs_f64() * 1000.0;
        let (library, library_debug) = materials::shader_library_with_jobs(
            &mut assets,
            &mut warnings,
            jobs,
            options.pbr_materials,
        )?;
        load_timings.shader_read_ms = library_debug.read_ms;
        load_timings.shader_parse_wall_ms = library_debug.parse_wall_ms;
        load_timings.shader_parse_cpu_ms = library_debug.parse_cpu_ms;

        // Workers idle while the parse finishes. Have a few open the retail
        // archives so the parallel texture reads that follow find ready handles
        // (opening one parses its whole central directory). Queued after the
        // shader library so it cannot delay shader parsing.
        for _ in 0..jobs.worker_count().saturating_sub(3).min(5) {
            let mut warm = assets.fork();
            jobs.submit(Task::MapPrepare, move || warm.warm_stock_archives())?;
        }
        let (parsed, parse_ms) = parse.join()?;
        load_timings.bsp_parse_ms = parse_ms;
        let (bsp, mesh) = parsed?;
        if options.steam_audio {
            if let Some(hit) = seed_acoustic.clone() {
                note_reused_stage(Some(jobs), &mut reused, "acoustic mesh", Task::MapAcoustics);
                steam_audio_sync = Some((Ok(hit), 0.0));
            } else {
                let acoustic_bsp = Arc::clone(&bsp);
                steam_audio_job = Some(jobs.submit(Task::MapAcoustics, move || {
                    let started = Instant::now();
                    let result = acoustic_bsp
                        .world_acoustic_mesh(4)
                        .map(Arc::new)
                        .map_err(|error| error.to_string());
                    (result, started.elapsed().as_secs_f64() * 1000.0)
                })?);
            }
        }
        (
            bsp,
            mesh,
            (seed_collision, movement, library, library_debug),
        )
    } else {
        let stage = Instant::now();
        let bsp = Arc::new(Bsp::parse(bytes.as_slice()).map_err(|e| e.to_string())?);
        let mesh = Arc::new(bsp.world_mesh(4).map_err(|e| e.to_string())?);
        load_timings.bsp_parse_ms = stage.elapsed().as_secs_f64() * 1000.0;

        let stage = Instant::now();
        let collision = if let Some(hit) = seed_collision {
            Some(hit)
        } else {
            match jka_movement::CollisionWorld::from_bsp(bytes.as_slice()) {
                Ok(world) => Some(world),
                Err(error) => {
                    warnings.push(format!("Player collision unavailable: {error}"));
                    None
                }
            }
        };
        load_timings.collision_ms = stage.elapsed().as_secs_f64() * 1000.0;

        if options.steam_audio {
            if let Some(hit) = seed_acoustic.clone() {
                note_reused_stage(jobs, &mut reused, "acoustic mesh", Task::MapAcoustics);
                steam_audio_sync = Some((Ok(hit), 0.0));
            } else {
                let acoustic_stage = Instant::now();
                let result = bsp
                    .world_acoustic_mesh(4)
                    .map(Arc::new)
                    .map_err(|error| error.to_string());
                steam_audio_sync = Some((result, acoustic_stage.elapsed().as_secs_f64() * 1000.0));
            }
        }

        let stage = Instant::now();
        let movement = load_movement(&mut assets, &mut warnings);
        load_timings.movement_ms = stage.elapsed().as_secs_f64() * 1000.0;
        let (library, library_debug) =
            materials::shader_library(&mut assets, &mut warnings, options.pbr_materials)?;
        load_timings.shader_read_ms = library_debug.read_ms;
        load_timings.shader_parse_wall_ms = library_debug.parse_wall_ms;
        load_timings.shader_parse_cpu_ms = library_debug.parse_cpu_ms;
        (bsp, mesh, (collision, movement, library, library_debug))
    };
    let (collision, movement, library, library_debug) = collision;
    let map_hash = match hash_job {
        Some(job) => job.join()?,
        None => inline_hash.expect("map hash computed inline without a hash job"),
    };
    let static_bsp_ao_cache = Some(static_bsp_ao_cache_info(root, game, name, map_hash));
    let steam_audio_bake_cache = options
        .steam_audio
        .then(|| crate::steam_audio::cache_info(root, game, name, map_hash));
    let mut phase_clock = prepare_started;
    phase_lap(&mut load_timings, 0, &mut phase_clock);
    let (inline_mesh, inline_batch_models) = bsp
        .inline_models_mesh(4)
        .map_err(|error| error.to_string())?;

    // Which shaders (and so which textures) the map uses needs only the meshes and
    // the shader library. Queue texture reads and decodes on the workers right now
    // so they overlap everything the loader does until `preload_finish`.
    let used: BTreeSet<_> = mesh
        .batches
        .iter()
        .chain(&inline_mesh.batches)
        .map(|batch| batch.shader)
        .collect();
    let used_shader_names: Vec<String> = used
        .iter()
        .filter_map(|&index| bsp.shaders.get(index))
        .map(|shader| String::from_utf8_lossy(&shader.name).to_ascii_lowercase())
        .collect();
    let mut textures = Textures::new();
    if let Some(seed) = seed {
        textures.set_seed(materials::TextureSeed::new(seed));
    }
    let texture_preload = if let Some(jobs) = jobs {
        let requests = materials::primary_texture_requests(
            &used_shader_names,
            &library,
            options.omit_environment_stages,
        );
        Some(textures.preload_start(&assets, &requests, jobs)?)
    } else {
        None
    };
    // By now the collision job has had the whole mesh build to finish.
    let collision = if let Some(job) = collision_pending {
        let (collision_result, collision_ms) = job.join()?;
        load_timings.collision_ms = collision_ms;
        match collision_result {
            Ok(world) => Some(world),
            Err(error) => {
                warnings.push(format!("Player collision unavailable: {error}"));
                None
            }
        }
    } else {
        collision
    };

    let physics_collision = if options.client_physics {
        bsp_physics_collision_mesh(&bsp, &mesh)
    } else {
        crate::cgame::ragdoll::PhysicsMapMesh::default()
    };
    let weather_occlusion = collision
        .as_ref()
        .and_then(|_| bsp_weather_occlusion_source(&bsp, &mesh));
    // R_MarkFragments reads the runtime shader's flags, so the scripts win over
    // the flags compiled into the BSP shader lump.
    let mark_shader_flags: Vec<(u32, u32)> = bsp
        .shaders
        .iter()
        .map(|shader| {
            let name = String::from_utf8_lossy(&shader.name).to_ascii_lowercase();
            library
                .get(&name)
                .map_or((shader.surface_flags, shader.contents), |script| {
                    (
                        script.collision_surface_flags_add,
                        script.collision_contents_add,
                    )
                })
        })
        .collect();
    let mark_surfaces = Arc::new(jka_assets::bsp::MarkSurfaces::from_bsp(
        &bsp,
        &mesh,
        &mark_shader_flags,
    ));

    let bsp_stats = BspMapStats {
        inline_model_midpoints: bsp
            .models
            .iter()
            .map(|model| std::array::from_fn(|i| (model.mins[i] + model.maxs[i]) * 0.5))
            .collect(),
        inline_model_bounds: bsp
            .models
            .iter()
            .map(|model| (model.mins, model.maxs))
            .collect(),
        brushes: bsp.brushes.len(),
        brush_sides: bsp.brush_sides.len(),
        planes: bsp.planes.len(),
        surfaces: bsp.surfaces.len(),
        vertices: bsp.vertices.len(),
        indices: bsp.indices.len(),
        collision_nodes: bsp.collision.as_ref().map_or(0, |tree| tree.nodes.len()),
        collision_leaves: bsp.collision.as_ref().map_or(0, |tree| tree.leaves.len()),
        pvs_clusters: bsp
            .visibility
            .as_ref()
            .map_or(0, |visibility| visibility.clusters),
    };
    let distance_cull = bsp_worldspawn_distance_cull(&bsp, &mut warnings);
    let sky_portal = bsp_sky_portal(&bsp);
    let global_fog_num = bsp_global_fog_num(&bsp);
    let global_fog = bsp_global_fog_params(&bsp, &library);
    if let Some(fog) = global_fog {
        devprintln!(
            1,
            "{name}: BSP global fog rgb=({:.3}, {:.3}, {:.3}) depth={:.1}",
            fog[0],
            fog[1],
            fog[2],
            fog[3]
        );
    }
    phase_lap(&mut load_timings, 1, &mut phase_clock);
    let lightmap_stage = Instant::now();
    let embedded_lightmap_pages = bsp.lightmaps.len() / (128 * 128 * 3);
    let mut referenced_lightmaps = external_lightmap_pages(&mesh);
    referenced_lightmaps.extend(external_lightmap_pages(&inline_mesh));
    let world_deluxe_mapping = embedded_deluxe_mapping(&mesh, embedded_lightmap_pages);
    let hdr_lightmaps_available = options.float_lightmap && hdr_lightmap_indexed(&assets, name);

    let mesh_uvs_bounded = |mesh: &jka_assets::bsp::Mesh| {
        mesh.batches.iter().all(|batch| {
            let slot = (0..4).find(|&i| batch.lightmaps[i] >= 0 && batch.lightmap_styles[i] < 254);
            slot.is_none_or(|slot| {
                mesh.indices[batch.indices.clone()].iter().all(|&index| {
                    mesh.vertices[index as usize].lightmap_uv[slot]
                        .iter()
                        .all(|v| (0.0..=1.0).contains(v))
                })
            })
        })
    };
    // Inline models share the world's lightmap pages, so they must also fit
    // the atlas before it can replace per-page lightmaps.
    let bounded_uvs = mesh_uvs_bounded(&mesh) && mesh_uvs_bounded(&inline_mesh);
    // An atlas cannot preserve the q3map2 lightmap/deluxemap pairing or FP16
    // companion values without building a second typed atlas. Keep the classic
    // atlas for ordinary 8-bit maps and use per-page textures for richer data.
    let atlas = (embedded_lightmap_pages > 0
        && bounded_uvs
        && !world_deluxe_mapping
        && !options.float_lightmap)
        .then(|| Atlas::new(&bsp.lightmaps));

    let mut external_lightmaps = Vec::new();
    let mut external_deluxemaps = Vec::new();
    let mut external_lightmap_lookup = BTreeMap::new();
    if embedded_lightmap_pages == 0 && !referenced_lightmaps.is_empty() {
        let paired_external = referenced_lightmaps.iter().all(|page| page % 2 == 0);
        let mut deluxe_count = 0usize;
        for &page in &referenced_lightmaps {
            let gpu_index = external_lightmaps.len();
            let image = load_external_lightmap(&mut assets, name, page, options.float_lightmap)?;
            let deluxe = try_load_external_deluxemap(&mut assets, name, page, paired_external)?;
            if deluxe.is_some() {
                deluxe_count += 1;
            }
            external_lightmaps.push(image);
            external_deluxemaps.push(
                deluxe
                    .unwrap_or_else(|| neutral_deluxemap(format!("{name} neutral deluxe {page}"))),
            );
            external_lightmap_lookup.insert(page, gpu_index);
        }
        warnings.push(format!(
            "{name}: using {} external lightmap page(s) from maps/{name}/lm_XXXX{}",
            external_lightmaps.len(),
            if deluxe_count != 0 {
                format!("; {deluxe_count} directional deluxemap companion(s)")
            } else {
                String::new()
            }
        ));
    } else if world_deluxe_mapping {
        warnings.push(format!(
            "{name}: q3map2/Rend2 embedded deluxemaps detected ({} lightmap + direction pairs)",
            embedded_lightmap_pages / 2
        ));
    }
    if options.float_lightmap {
        warnings.push(if hdr_lightmaps_available {
            format!(
                "{name}: r_floatLightmap: HDR lm_XXXX.hdr companions detected; preserving baked lighting in FP16"
            )
        } else {
            format!(
                "{name}: r_floatLightmap: using FP16 lightmap storage (no HDR companion set detected)"
            )
        });
    }
    let lightmap_pages = if embedded_lightmap_pages > 0 {
        if world_deluxe_mapping {
            embedded_lightmap_pages / 2
        } else if atlas.is_some() {
            1
        } else {
            embedded_lightmap_pages
        }
    } else {
        external_lightmaps.len()
    };
    let static_light_grid = prepare_static_light_grid(&bsp, &mut assets, name, &mut warnings)?;
    load_timings.lightmap_ms = lightmap_stage.elapsed().as_secs_f64() * 1000.0;
    phase_lap(&mut load_timings, 2, &mut phase_clock);

    // Spawns, FX runners, brush entities and the entity graph read only the BSP,
    // so they run here while the workers are still decoding textures.
    let static_models = Arc::new(bsp.static_models());
    let mut spawns: Vec<_> = bsp
        .deathmatch_spawns()
        .into_iter()
        .map(|spawn| SpawnPoint {
            position: {
                let mut p = render_position(spawn.origin);
                p[1] += 9.0;
                p
            },
            yaw: spawn.yaw.to_radians(),
            initial: spawn.initial,
            no_humans: spawn.no_humans,
        })
        .collect();
    if spawns.is_empty() {
        let model = &bsp.models[0];
        let center = std::array::from_fn(|i| (model.mins[i] + model.maxs[i]) * 0.5);
        let mut position = render_position(center);
        position[1] += 9.0;
        spawns.push(SpawnPoint {
            position,
            yaw: 0.0,
            ..SpawnPoint::default()
        });
    }
    let fx_runners = bsp_fx_runners(&bsp, &mut warnings);
    let brush_entities = bsp_brush_entities(&bsp);
    let entity_graph = Some(Arc::new(crate::entity_graph::EntityGraph::build(&bsp)));
    phase_lap(&mut load_timings, 3, &mut phase_clock);
    if let Some(preload) = texture_preload {
        // Everything above overlapped the workers; this is only the remainder.
        textures.preload_finish(preload)?;
    }
    phase_lap(&mut load_timings, 4, &mut phase_clock);
    let material_stage = Instant::now();
    if let Some(jobs) = jobs {
        jobs.mark_started(Task::MapMaterials);
    }
    let sun = authored_sun(
        used_shader_names.iter().map(String::as_str),
        &library,
        &mut warnings,
    );
    let describe_started = Instant::now();
    let stats_before = (
        textures.load_stats.read_ms,
        textures.load_stats.decode_ms,
        textures.load_stats.mip_ms,
        textures.load_stats.decoded_images,
        textures.images.len(),
    );
    let surface_materials: Vec<_> = bsp
        .shaders
        .iter()
        .enumerate()
        .map(|(index, shader)| {
            if !used.contains(&index) {
                return SurfaceMaterial {
                    hidden: true,
                    ..Default::default()
                };
            }
            let shader_name = String::from_utf8_lossy(&shader.name).to_ascii_lowercase();
            materials::describe(
                &shader_name,
                shader.surface_flags,
                library.get(&shader_name),
                library_debug.origins.get(&shader_name),
                &mut assets,
                &mut textures,
                options.gen_normal_maps,
                options.omit_environment_stages,
            )
        })
        .collect();
    let describe_ms = describe_started.elapsed().as_secs_f64() * 1000.0;
    let tints_started = Instant::now();

    let grass_ground_tints: Vec<[u8; 4]> = surface_materials
        .iter()
        .map(|material| {
            material
                .grass
                .map(|_| grass_ground_albedo_tint(material, &textures))
                .unwrap_or([0, 0, 0, 0])
        })
        .collect();
    let tints_ms = tints_started.elapsed().as_secs_f64() * 1000.0;

    let lights_started = Instant::now();
    let (surface_lights, surface_light_candidates) = bsp_surface_lights(&mesh, &surface_materials);
    let surface_lights_ms = lights_started.elapsed().as_secs_f64() * 1000.0;

    let debug_started = Instant::now();
    let material_debug = material_debug_info(
        &library_debug,
        used.iter().filter_map(|&index| {
            let shader = bsp.shaders.get(index)?;
            let name = String::from_utf8_lossy(&shader.name).to_ascii_lowercase();
            Some((name, surface_materials.get(index)?.clone()))
        }),
        &library,
        &textures,
    );
    load_timings.material_ms = material_stage.elapsed().as_secs_f64() * 1000.0;
    devprintln!(
        2,
        "[MAP MATERIALS] total {:.1} ms | describe {:.1} (of which texture read/probe {:.1}, decode {:.1}, mip {:.1}; {} decoded, {} new image slot(s); light-image averaging {:.1}) | grass tints {:.1} | surface lights {:.1} | material debug {:.1}",
        load_timings.material_ms,
        describe_ms,
        textures.load_stats.read_ms - stats_before.0,
        textures.load_stats.decode_ms - stats_before.1,
        textures.load_stats.mip_ms - stats_before.2,
        textures.load_stats.decoded_images - stats_before.3,
        textures.images.len() - stats_before.4,
        materials::LIGHT_IMAGE_AVERAGE_NS.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 1.0e6,
        tints_ms,
        surface_lights_ms,
        debug_started.elapsed().as_secs_f64() * 1000.0,
    );
    if let Some(jobs) = jobs {
        jobs.mark_done(Task::MapMaterials);
    }
    phase_lap(&mut load_timings, 5, &mut phase_clock);

    let surface_sprite_effects =
        collect_surface_sprite_effect_emitters(&bsp, &mesh, &surface_materials);
    if !surface_sprite_effects.is_empty() {
        let triangle_count: usize = surface_sprite_effects
            .iter()
            .map(|emitter| emitter.triangles.len())
            .sum();
        devprintln!(
            1,
            "{name}: surfaceSprites effect: {} emitter stage(s), {triangle_count} source triangle(s)",
            surface_sprite_effects.len()
        );
    }

    // Grass blade generation is independent of ordinary BSP vertex packing once
    // emitter triangles/material metadata have been captured. Start it on the map
    // worker pool before entering the main geometry walk so both jobs overlap.
    let grass_stage = Instant::now();
    let (grass_fog_slots, grass_local_fogs) = if options.grass {
        grass_local_fog_slots(&bsp, &library, global_fog_num)
    } else {
        (BTreeMap::new(), vec![[0.0; 4]])
    };
    let (grass_emitters, grass_signatures) = if options.grass {
        collect_grass_emitters(
            &bsp,
            &mesh,
            &surface_materials,
            &grass_ground_tints,
            &grass_fog_slots,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let mut grass_jobs = Vec::new();
    let mut grass_sync = None;
    // Blade generation is a pure function of the emitter triangles and the
    // lightmap pixels they sample, so an unchanged fingerprint reuses the patches.
    let mut grass_key = None;
    let mut grass_memo_hit = None;
    let grass_lightmaps = (!grass_emitters.is_empty()).then(|| {
        Arc::new(grass_lightmap_sources(
            &bsp,
            &external_lightmaps,
            &external_lightmap_lookup,
        ))
    });
    if let Some(lightmaps) = &grass_lightmaps {
        let key = grass_fingerprint(&grass_emitters, &grass_signatures, lightmaps);
        grass_key = Some(key);
        grass_memo_hit = seed
            .filter(|seed| seed.stage_keys.grass == Some(key))
            .map(|seed| {
                let blades = seed
                    .grass_patches
                    .iter()
                    .map(|patch| patch.instances.len())
                    .sum();
                (seed.grass_patches.clone(), blades)
            });
        if grass_memo_hit.is_some() {
            note_reused_stage(jobs, &mut reused, "grass", Task::MapGrass);
        }
    }
    if let Some(lightmaps) = grass_lightmaps.filter(|_| grass_memo_hit.is_none()) {
        let clump_data = crate::grass::godot_clump_noise();
        if let Some(jobs) = jobs {
            let worker_count = jobs.worker_count().min(grass_emitters.len()).max(1);
            let chunk_size = grass_emitters.len().div_ceil(worker_count);
            let mut iter = grass_emitters.into_iter();
            loop {
                let chunk = iter.by_ref().take(chunk_size).collect::<Vec<_>>();
                if chunk.is_empty() {
                    break;
                }
                let lightmaps = Arc::clone(&lightmaps);
                let clump_data = Arc::clone(&clump_data);
                grass_jobs.push(jobs.submit(Task::MapGrass, move || {
                    let started = Instant::now();
                    let result = generate_grass_chunk(chunk, lightmaps, clump_data);
                    (result, started.elapsed().as_secs_f64() * 1000.0)
                })?);
            }
        } else {
            let started = Instant::now();
            let result = generate_grass_chunk(grass_emitters, lightmaps, clump_data);
            load_timings.grass_cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
            grass_sync = Some(result);
        }
    }

    phase_lap(&mut load_timings, 6, &mut phase_clock);
    let geometry_stage = Instant::now();
    if let Some(jobs) = jobs {
        jobs.mark_started(Task::MapGeometry);
    }
    // Atomic so `batch_geometry` is `Fn` and the world walk can run it on rayon.
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    let invisible_render_culled_batches = AtomicUsize::new(0);
    let invisible_render_culled_triangles = AtomicUsize::new(0);
    let invisible_render_culled_vertices = AtomicUsize::new(0);
    // One BSP mesh batch -> its material/lightmap group and render vertices.
    // Shared by the world and inline models so a mover's surfaces resolve
    // lightmaps, vertex lighting and fog exactly like the world's.
    let batch_geometry = |mesh: &jka_assets::bsp::Mesh,
                          batch_index: usize,
                          batch: &jka_assets::bsp::DrawBatch|
     -> Option<(GroupKey, WorldGeometryChunk)> {
        let material = &surface_materials[batch.shader];
        if material.hidden {
            return None;
        }
        let surface = &bsp.surfaces[batch.surface];
        let lightmap_slot =
            (0..4).find(|&i| batch.lightmaps[i] >= 0 && batch.lightmap_styles[i] < 254);
        let vertex_lit_slot = (0..4)
            .find(|&i| batch.lightmaps[i] == LIGHTMAP_BY_VERTEX && surface.vertex_styles[i] < 254);
        let vertex_lit = vertex_lit_slot.is_some();
        if material_render_is_guaranteed_discarded(material, vertex_lit) {
            invisible_render_culled_batches.fetch_add(1, AtomicOrdering::Relaxed);
            invisible_render_culled_triangles
                .fetch_add(batch.indices.len() / 3, AtomicOrdering::Relaxed);
            // World geometry is expanded to one GPU vertex per source index,
            // so this is the exact number of render vertices we avoid packing.
            invisible_render_culled_vertices
                .fetch_add(batch.indices.len(), AtomicOrdering::Relaxed);
            return None;
        }
        let class = class_for(material);
        let source_lightmap = lightmap_slot
            .filter(|_| !material.sky)
            .map(|i| batch.lightmaps[i] as usize);
        let lightmap = source_lightmap.map(|page| {
            if atlas.is_some() {
                0
            } else if !external_lightmap_lookup.is_empty() {
                external_lightmap_lookup[&page]
            } else if world_deluxe_mapping {
                page >> 1
            } else {
                page
            }
        });
        let color_slot = vertex_lit_slot
            .or(lightmap_slot)
            .or_else(|| (0..4).find(|&i| surface.vertex_styles[i] < 254))
            .unwrap_or(0) as u8;
        let mut geometry = Vec::with_capacity(batch.indices.len());
        for triangle in mesh.indices[batch.indices.clone()]
            .as_chunks::<3>()
            .0
            .iter()
        {
            let source_indices = [
                triangle[0] as usize,
                triangle[2] as usize,
                triangle[1] as usize,
            ];

            for vertex_index in source_indices {
                let vertex = mesh.vertices[vertex_index];
                let raw_lightmap_uv = lightmap_slot
                    .map(|i| vertex.lightmap_uv[i])
                    .unwrap_or([-1.0, -1.0]);
                let lightmap_uv = match (&atlas, source_lightmap) {
                    (Some(atlas), Some(page)) => atlas.uv(page, raw_lightmap_uv),
                    (None, Some(_)) => raw_lightmap_uv,
                    _ => [-1.0, -1.0],
                };
                let color = vertex.color[color_slot as usize].map(|v| f32::from(v) / 255.0);
                geometry.push(GpuVertex {
                    position: render_position(vertex.position),
                    uv: vertex.texcoord,
                    lightmap_uv,
                    normal: render_position(vertex.normal),
                    color,
                    alpha_cutoff: 1.0,
                });
            }
        }

        let has_environment_stage = material
            .stages
            .iter()
            .any(|stage| matches!(stage.tc_gen, TcGen::Environment));
        let planar_group = if options.planar_environment
            && has_environment_stage
            && class != DrawClass::Transparent
            && !material.planar_reflection
        {
            planar_group_key(&geometry)
        } else {
            None
        };
        let preserve_unique_plane = options.planar_reflections
            && (material.planar_reflection
                || (options.planar_environment && has_environment_stage && planar_group.is_none()));
        let key = GroupKey {
            class,
            shader: batch.shader,
            lightmap,
            vertex_lit,
            color_slot,
            fog_num: surface.fog_num,
            // Surfaces of one shader/lightmap/fog state share a group, like
            // OpenJK's per-shader batches, so transparent surfaces collapse into
            // one draw per stage instead of one per BSP surface (groups are
            // still ordered by shader). Planar reflection surfaces only need
            // isolation while the reflection topology is enabled; tcGen
            // environment geometry that is itself planar is grouped by plane
            // instead of by individual BSP surface.
            transparent_order: if preserve_unique_plane {
                batch_index + 1
            } else {
                0
            },
            planar_group,
        };
        let surface_ids =
            vec![u32::try_from(batch.surface).unwrap_or(u32::MAX); geometry.len() / 3];
        Some((
            key,
            WorldGeometryChunk {
                vertices: geometry,
                surface_ids,
            },
        ))
    };

    // OpenJK performs coarse dlight rejection on BSP surfaces before projected
    // lighting. Retain equivalent immutable surface data so the WGPU path can
    // build a small per-surface bitmask each frame instead of testing every
    // runtime light in every fragment.
    let dlight_stage = Instant::now();
    let legacy_dlight_surfaces = bsp
        .surfaces
        .iter()
        .map(|surface| {
            let material = &surface_materials[surface.shader];
            let shader_flags = bsp
                .shaders
                .get(surface.shader)
                .map_or(0, |shader| shader.surface_flags);
            let eligible = !material.hidden
                && !material.sky
                && shader_flags & jka_assets::bsp::SURF_NODLIGHT == 0
                && !matches!(surface.kind, SurfaceKind::Flare);
            if !eligible {
                return LegacyDlightSurface::default();
            }

            let mut bounds_min = [f32::INFINITY; 3];
            let mut bounds_max = [f32::NEG_INFINITY; 3];
            for vertex in &bsp.vertices[surface.vertices.clone()] {
                let p = render_position(vertex.position);
                for axis in 0..3 {
                    bounds_min[axis] = bounds_min[axis].min(p[axis]);
                    bounds_max[axis] = bounds_max[axis].max(p[axis]);
                }
            }
            if !bounds_min[0].is_finite() {
                bounds_min = [0.0; 3];
                bounds_max = [0.0; 3];
            }

            let plane = if matches!(surface.kind, SurfaceKind::Planar) {
                surface
                    .vertices
                    .clone()
                    .find_map(|index| {
                        let vertex = bsp.vertices.get(index)?;
                        let normal =
                            Vec3::from_array(render_position(vertex.normal)).normalize_or_zero();
                        if normal.length_squared() <= 1.0e-10 {
                            return None;
                        }
                        let point = Vec3::from_array(render_position(vertex.position));
                        Some([normal.x, normal.y, normal.z, normal.dot(point)])
                    })
                    .unwrap_or([0.0; 4])
            } else {
                [0.0; 4]
            };

            LegacyDlightSurface {
                cull_kind: match surface.kind {
                    SurfaceKind::Planar
                        if plane[0] != 0.0 || plane[1] != 0.0 || plane[2] != 0.0 =>
                    {
                        1
                    }
                    SurfaceKind::Patch | SurfaceKind::Triangles | SurfaceKind::Planar => 2,
                    SurfaceKind::Flare => 0,
                },
                plane,
                bounds_min,
                bounds_max,
            }
        })
        .collect::<Vec<_>>();
    let dlight_surfaces_ms = dlight_stage.elapsed().as_secs_f64() * 1000.0;

    let mut groups: BTreeMap<GroupKey, Geometry> = BTreeMap::new();
    let mut triangles = 0usize;
    let walk_stage = Instant::now();
    let mut signature_time = std::time::Duration::ZERO;
    // Each BSP batch expands to its render vertices and PVS signature
    // independently, so that runs on rayon; the results come back in batch order
    // and are merged sequentially, which keeps every group's contents identical.
    let walked = {
        use rayon::prelude::*;
        mesh.batches
            .par_iter()
            .enumerate()
            .map(|(batch_index, batch)| {
                let (key, chunk) = batch_geometry(&mesh, batch_index, batch)?;
                let signature_started = Instant::now();
                let signature = bsp
                    .visibility
                    .as_ref()
                    .map(|vis| pvs_signature(vis, &vis.surface_clusters[batch.surface]))
                    .unwrap_or_default();
                let signature_elapsed = signature_started.elapsed();
                let area_signature = bsp
                    .visibility
                    .as_ref()
                    .and_then(|vis| vis.surface_area_masks.get(batch.surface).copied())
                    .unwrap_or([0_u64; 4]);
                Some((key, chunk, signature, area_signature, signature_elapsed))
            })
            .collect::<Vec<_>>()
    };
    for (key, batch_geometry, signature, area_signature, signature_elapsed) in
        walked.into_iter().flatten()
    {
        signature_time += signature_elapsed;
        triangles += batch_geometry.vertices.len() / 3;
        let chunk = groups
            .entry(key)
            .or_default()
            .by_pvs_signature
            .entry((signature, area_signature))
            .or_default();
        chunk.vertices.extend_from_slice(&batch_geometry.vertices);
        chunk
            .surface_ids
            .extend_from_slice(&batch_geometry.surface_ids);
    }
    let walk_ms = walk_stage.elapsed().as_secs_f64() * 1000.0;

    let pack_stage = Instant::now();
    // Ordering is a pure function of one group's pieces, so every group is
    // ordered at once; the groups stay in key order for the sequential pack below.
    let order_started = Instant::now();
    let ordered_groups = {
        use rayon::prelude::*;
        groups
            .into_iter()
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(key, geometry)| (key, order_pvs_pieces(geometry.by_pvs_signature)))
            .collect::<Vec<_>>()
    };
    let order_time = order_started.elapsed();
    let mut vertices = Vec::new();
    let mut legacy_dlight_triangle_surfaces = Vec::new();
    let mut batches = Vec::new();
    let mut pvs_batches = Vec::new();
    for (key, ordered_pieces) in ordered_groups {
        let material = &surface_materials[key.shader];
        let material_debug_index = bsp.shaders.get(key.shader).and_then(|shader| {
            let shader_name = String::from_utf8_lossy(&shader.name);
            material_debug
                .entries
                .iter()
                .position(|entry| entry.name.eq_ignore_ascii_case(shader_name.as_ref()))
        });
        let coarse_start =
            u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;
        let mut coarse_signature = Vec::<u64>::new();
        let mut coarse_area_signature = [0_u64; 4];

        for ((signature, area_signature), piece) in ordered_pieces {
            let piece_start =
                u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;

            let (fog, fog_is_global) = resolved_bsp_surface_fog(
                &bsp,
                &library,
                material,
                key.fog_num,
                global_fog_num,
                global_fog,
            );
            vertices.extend_from_slice(&piece.vertices);
            legacy_dlight_triangle_surfaces.extend_from_slice(&piece.surface_ids);
            let piece_end =
                u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;

            if coarse_signature.len() < signature.len() {
                coarse_signature.resize(signature.len(), 0);
            }
            for (word, value) in signature.iter().enumerate() {
                coarse_signature[word] |= value;
            }
            for word in 0..coarse_area_signature.len() {
                coarse_area_signature[word] |= area_signature[word];
            }

            // FULL/AUTO normally split a MINIMAL batch by exact PVS signature.
            // Do not do that for sky: the authored BSP sky polygons are the
            // screen-space admission mask for the skybox, so removing only one
            // PVS piece can cut a rectangular hole through an otherwise visible
            // sky. MINIMAL already has the correct conservative behavior: if any
            // constituent sky surface is visible, submit the whole coarse sky
            // geometry. Build FULL/AUTO the same way for this domain, with the
            // ORed signature emitted below after every piece has contributed.
            if !material.sky {
                append_material_batches(
                    &mut pvs_batches,
                    material,
                    key.shader,
                    material_debug_index,
                    key.vertex_lit,
                    piece_start..piece_end,
                    key.lightmap,
                    &signature,
                    area_signature,
                    fog,
                    fog_is_global,
                );
            }
        }

        let coarse_end =
            u32::try_from(vertices.len()).map_err(|_| "world vertex count exceeds u32")?;
        let (fog, fog_is_global) = resolved_bsp_surface_fog(
            &bsp,
            &library,
            material,
            key.fog_num,
            global_fog_num,
            global_fog,
        );
        append_material_batches(
            &mut batches,
            material,
            key.shader,
            material_debug_index,
            key.vertex_lit,
            coarse_start..coarse_end,
            key.lightmap,
            &coarse_signature,
            coarse_area_signature,
            fog,
            fog_is_global,
        );
        if material.sky {
            append_material_batches(
                &mut pvs_batches,
                material,
                key.shader,
                material_debug_index,
                key.vertex_lit,
                coarse_start..coarse_end,
                key.lightmap,
                &coarse_signature,
                coarse_area_signature,
                fog,
                fog_is_global,
            );
        }
    }
    load_timings.geometry_detail_ms = [
        dlight_surfaces_ms,
        walk_ms,
        signature_time.as_secs_f64() * 1000.0,
        pack_stage.elapsed().as_secs_f64() * 1000.0,
        order_time.as_secs_f64() * 1000.0,
    ];

    let mut inline_vertices = Vec::new();
    let mut inline_batches = Vec::new();
    let mut inline_models = Vec::new();
    {
        let mut start = 0usize;
        while start < inline_mesh.batches.len() {
            let model = inline_batch_models[start];
            let end = inline_batch_models[start..]
                .iter()
                .position(|&other| other != model)
                .map_or(inline_mesh.batches.len(), |offset| start + offset);
            let mut model_groups: BTreeMap<GroupKey, Vec<GpuVertex>> = BTreeMap::new();
            for batch_index in start..end {
                let batch = &inline_mesh.batches[batch_index];
                if let Some((key, batch_geometry)) =
                    batch_geometry(&inline_mesh, batch_index, batch)
                {
                    model_groups
                        .entry(key)
                        .or_default()
                        .extend_from_slice(&batch_geometry.vertices);
                }
            }
            let vertex_start = u32::try_from(inline_vertices.len())
                .map_err(|_| "inline vertex count exceeds u32")?;
            let batch_start = inline_batches.len();
            for (key, group_vertices) in model_groups {
                let material = &surface_materials[key.shader];
                let material_debug_index = bsp.shaders.get(key.shader).and_then(|shader| {
                    let shader_name = String::from_utf8_lossy(&shader.name);
                    material_debug
                        .entries
                        .iter()
                        .position(|entry| entry.name.eq_ignore_ascii_case(shader_name.as_ref()))
                });
                let range_start = u32::try_from(inline_vertices.len())
                    .map_err(|_| "inline vertex count exceeds u32")?;
                inline_vertices.extend_from_slice(&group_vertices);
                let range_end = u32::try_from(inline_vertices.len())
                    .map_err(|_| "inline vertex count exceeds u32")?;
                let (fog, fog_is_global) = resolved_bsp_surface_fog(
                    &bsp,
                    &library,
                    material,
                    key.fog_num,
                    global_fog_num,
                    global_fog,
                );
                append_material_batches(
                    &mut inline_batches,
                    material,
                    key.shader,
                    material_debug_index,
                    key.vertex_lit,
                    range_start..range_end,
                    key.lightmap,
                    &[],
                    [0_u64; 4],
                    fog,
                    fog_is_global,
                );
            }
            // A moving brush cannot be promoted to the ocean clipmap, and
            // mirrors are bound to static authored planes.
            for batch in &mut inline_batches[batch_start..] {
                batch.water_primary = false;
                batch.planar_reflection = false;
                batch.planar_environment_candidate = false;
            }
            let vertex_end = u32::try_from(inline_vertices.len())
                .map_err(|_| "inline vertex count exceeds u32")?;
            if batch_start != inline_batches.len() {
                inline_models.push(InlineModelGeometry {
                    model,
                    vertices: vertex_start..vertex_end,
                    batches: batch_start..inline_batches.len(),
                });
            }
            start = end;
        }
    }
    if !inline_models.is_empty() {
        devprintln!(
            1,
            "{name}: {} drawable inline BSP model(s), {} vertices, {} material batch(es)",
            inline_models.len(),
            inline_vertices.len(),
            inline_batches.len()
        );
    }
    let invisible_render_culled_batches = invisible_render_culled_batches.into_inner();
    if invisible_render_culled_batches > 0 {
        devprintln!(
            2,
            "{name}: guaranteed invisible/no-op render cull: {invisible_render_culled_batches} BSP batch(es), {} triangle(s), {} GPU render vertices omitted; source BSP geometry retained for semantic extraction",
            invisible_render_culled_triangles.into_inner(),
            invisible_render_culled_vertices.into_inner(),
        );
    }

    phase_lap(&mut load_timings, 7, &mut phase_clock);
    let mut lights = bsp_dynamic_lights(&bsp);
    let entity_light_count = lights.len();
    let surface_light_count = surface_lights.len();
    lights.extend(surface_lights);
    if surface_light_candidates > 0 {
        devprintln!(
            1,
            "{name}: emissive/area lights: {surface_light_count} real-time sample(s) selected from {surface_light_candidates} candidate(s); {entity_light_count} entity light(s) retained first"
        );
    }
    // GI reads immutable world geometry/lights only. Clone the modest render mesh
    // snapshot and let a map worker voxelize/propagate while the loader continues
    // reflection/planar/lightmap/spawn finalization on the map-loader thread.
    let gi_key = options.voxel_probe_gi.then(|| {
        let mut fingerprint = StageFingerprint::new("voxel-probe-gi");
        fingerprint
            .pod(&vertices)
            .draw_batches(&mut batches)
            .debug(&lights)
            .debug(&sun);
        fingerprint.finish()
    });
    let gi_memo_hit = gi_key.and_then(|key| {
        seed.filter(|seed| seed.stage_keys.gi == Some(key))
            .map(|seed| seed.voxel_probe_gi.clone())
    });
    if gi_memo_hit.is_some() {
        note_reused_stage(jobs, &mut reused, "voxel GI", Task::MapGi);
    }
    // Without a worker pool GI runs right here: the geometry vectors are handed to
    // the PVS plan job further down, so nothing after this point may read them.
    let mut gi_sync = None;
    let gi_job = if options.voxel_probe_gi && gi_memo_hit.is_none() {
        if let Some(jobs) = jobs {
            let gi_vertices = vertices.clone();
            let gi_batches = batches.clone();
            let gi_lights = lights.clone();
            Some(jobs.submit(Task::MapGi, move || {
                let started = Instant::now();
                let grid = build_voxel_probe_gi(&gi_vertices, &gi_batches, &gi_lights, sun);
                (grid, started.elapsed().as_secs_f64() * 1000.0)
            })?)
        } else {
            let gi_stage = Instant::now();
            let grid = build_voxel_probe_gi(&vertices, &batches, &lights, sun);
            gi_sync = Some((grid, gi_stage.elapsed().as_secs_f64() * 1000.0));
            None
        }
    } else {
        None
    };

    let reflection_probes = load_reflection_probes(&bsp, &mut assets, name, &mut warnings)?;
    assign_reflection_probes(&mut batches, &vertices, &reflection_probes);
    assign_reflection_probes(&mut pvs_batches, &vertices, &reflection_probes);
    assign_reflection_probes(&mut inline_batches, &inline_vertices, &reflection_probes);
    let (portal_anchors, camera_portal_count) = bsp_portal_surface_anchors(&bsp);
    let authored_portal_batch_count;
    if options.planar_reflections {
        assign_planar_reflection_planes(&mut batches, &vertices, options.planar_environment);
        assign_planar_reflection_planes(&mut pvs_batches, &vertices, options.planar_environment);
        authored_portal_batch_count = batches
            .iter()
            .filter(|batch| batch.planar_reflection)
            .count();
        retain_authored_planar_mirrors(&mut batches, &portal_anchors);
        retain_authored_planar_mirrors(&mut pvs_batches, &portal_anchors);
    } else {
        authored_portal_batch_count = 0;
        // Reflection quality is restart-latched specifically so the map can be
        // prepared without per-plane metadata when planar reflections are off.
        // Clear the material-authored eligibility bits as well; otherwise later
        // draw-compaction sees them and still refuses to merge compatible draws.
        for batch in batches.iter_mut().chain(pvs_batches.iter_mut()) {
            batch.planar_reflection = false;
            batch.planar_environment_candidate = false;
            batch.planar_plane = [0.0; 4];
        }
    }
    cache_reflection_decisions(&mut batches);
    cache_reflection_decisions(&mut pvs_batches);
    log_reflection_cache(name, &batches);
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
    load_timings.geometry_ms = geometry_stage.elapsed().as_secs_f64() * 1000.0;
    if let Some(jobs) = jobs {
        jobs.mark_done(Task::MapGeometry);
    }
    phase_lap(&mut load_timings, 8, &mut phase_clock);

    // The authored ocean planes were the last thing that edited the batch lists
    // before the plan, and they read only the BSP, so they run here and the plan
    // can start now instead of after lightmaps, grass, GI and audio.
    let authored_oceans = crate::ocean::authoring::AuthoredOcean::from_bsp(&bsp);
    append_authored_ocean_planes(
        &authored_oceans,
        &mut vertices,
        &mut batches,
        &mut pvs_batches,
    );

    // AUTO 4 is pure CPU map preprocessing. Keep its potentially expensive
    // per-cluster PVS/group search off the renderer thread and expose it as its
    // own loading-screen stage. The plan reads only batch draw state, PVS/area
    // signatures and vertex positions (never decoded textures, GI, grass or
    // lightmap pixels), so the worker is started as soon as those are final and
    // joined after the unrelated stages below. It owns the vectors in the
    // meantime and returns them unchanged alongside the immutable draw plan.
    let visibility = bsp.visibility.clone();
    let plan_wanted = visibility.is_some() && !pvs_batches.is_empty();
    // The plan reads the draw-state of both batch lists and the vertex positions
    // (for bounds); visibility is fixed by the BSP bytes the memo is keyed on.
    let portal_key = plan_wanted.then(|| {
        let mut fingerprint = StageFingerprint::new("auto4-portal-plan");
        fingerprint
            .draw_batches(&mut batches)
            .draw_batches(&mut pvs_batches)
            .positions(&vertices);
        fingerprint.finish()
    });
    let portal_memo_hit = portal_key.and_then(|key| {
        seed.filter(|seed| seed.stage_keys.portal == Some(key))
            .map(|seed| seed.portal_draw_plan.clone())
    });
    if portal_memo_hit.is_some() {
        note_reused_stage(jobs, &mut reused, "PVS plans", Task::MapPortalPlans);
    }
    let mut portal_inputs = Some((vertices, batches, pvs_batches, visibility));
    let mut portal_job = None;
    if portal_memo_hit.is_none() && plan_wanted {
        if let Some(jobs) = jobs {
            let (vertices, batches, pvs_batches, visibility) =
                portal_inputs.take().expect("plan inputs are taken once");
            let total = u32::try_from(visibility.as_ref().map_or(0, |vis| vis.clusters))
                .unwrap_or(u32::MAX)
                .max(1);
            portal_job =
                Some(
                    jobs.submit_progress(Task::MapPortalPlans, total, move |progress| {
                        let started = Instant::now();
                        let plan = build_prepared_portal_draw_plan(
                            &batches,
                            &pvs_batches,
                            visibility
                                .as_ref()
                                .expect("visibility checked before AUTO 4 job"),
                            &vertices,
                            |completed| progress.set_completed(completed),
                        );
                        let plan_ms = started.elapsed().as_secs_f64() * 1000.0;
                        (vertices, batches, pvs_batches, visibility, plan, plan_ms)
                    })?,
                );
        }
    }
    phase_lap(&mut load_timings, 9, &mut phase_clock);

    let (lightmaps, deluxemaps) = if !external_lightmaps.is_empty() {
        (external_lightmaps, external_deluxemaps)
    } else if let Some(atlas) = atlas {
        (
            vec![TextureData {
                label: "JKA lightmap atlas".into(),
                source: None,
                width: atlas.width,
                height: atlas.height,
                rgba: atlas.rgba,
                rgba16f: None,
                mip_level_count: 1,
                clamp: true,
                srgb: true,
            }],
            vec![neutral_deluxemap("JKA atlas neutral deluxe")],
        )
    } else {
        let pages = bsp.lightmaps.as_chunks::<{ 128 * 128 * 3 }>().0;
        let mut lightmaps = Vec::with_capacity(lightmap_pages);
        let mut deluxemaps = Vec::with_capacity(lightmap_pages);
        if world_deluxe_mapping {
            for effective in 0..lightmap_pages {
                let source_page = effective * 2;
                let mut lightmap = if hdr_lightmaps_available {
                    try_load_hdr_lightmap(&mut assets, name, source_page)?.unwrap_or_else(|| {
                        embedded_lightmap_texture(source_page, &pages[source_page])
                    })
                } else {
                    embedded_lightmap_texture(source_page, &pages[source_page])
                };
                if options.float_lightmap {
                    materials::promote_lightmap_to_float(&mut lightmap);
                }
                lightmaps.push(lightmap);
                deluxemaps.push(embedded_deluxemap_texture(
                    source_page + 1,
                    &pages[source_page + 1],
                ));
            }
        } else {
            for (source_page, page) in pages.iter().enumerate() {
                let mut lightmap = if hdr_lightmaps_available {
                    try_load_hdr_lightmap(&mut assets, name, source_page)?
                        .unwrap_or_else(|| embedded_lightmap_texture(source_page, page))
                } else {
                    embedded_lightmap_texture(source_page, page)
                };
                if options.float_lightmap {
                    materials::promote_lightmap_to_float(&mut lightmap);
                }
                lightmaps.push(lightmap);
                deluxemaps.push(neutral_deluxemap(format!(
                    "JKA neutral deluxe {source_page}"
                )));
            }
        }
        (lightmaps, deluxemaps)
    };

    phase_lap(&mut load_timings, 10, &mut phase_clock);
    // Grass generation jobs were launched before the main geometry walk. Resolve and
    // merge them before joining GI so patch-finalization jobs can use the remaining
    // map workers while the queued/running GI job occupies at most one worker.
    let mut grass_groups = BTreeMap::<GrassPatchKey, Vec<GrassInstance>>::new();
    let mut grass_blades = 0usize;
    if let Some((groups, blades)) = grass_sync {
        grass_blades += blades;
        for (worker_key, mut instances) in groups {
            let key = GrassPatchKey {
                cell_x: worker_key.cell_x,
                cell_z: worker_key.cell_z,
                pvs_signature: grass_signatures
                    .get(worker_key.signature_id as usize)
                    .cloned()
                    .unwrap_or_default(),
            };
            match grass_groups.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(instances);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().append(&mut instances);
                }
            }
        }
    }
    for handle in grass_jobs {
        let ((groups, blades), cpu_ms) = handle.join()?;
        load_timings.grass_cpu_ms += cpu_ms;
        grass_blades += blades;
        for (worker_key, mut instances) in groups {
            let key = GrassPatchKey {
                cell_x: worker_key.cell_x,
                cell_z: worker_key.cell_z,
                pvs_signature: grass_signatures
                    .get(worker_key.signature_id as usize)
                    .cloned()
                    .unwrap_or_default(),
            };
            match grass_groups.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(instances);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().append(&mut instances);
                }
            }
        }
    }
    let grass_patches = if let Some((patches, blades)) = grass_memo_hit {
        grass_blades = blades;
        patches
    } else {
        let (patches, grass_finalize_cpu_ms) = finish_grass_patches_with_jobs(grass_groups, jobs)?;
        load_timings.grass_cpu_ms += grass_finalize_cpu_ms;
        patches
    };
    load_timings.grass_ms = grass_stage.elapsed().as_secs_f64() * 1000.0;
    if grass_blades != 0 {
        let grass_cpu_bytes = grass_blades * std::mem::size_of::<GrassInstance>();
        devprintln!(
            1,
            "{name}: procedural grass: {grass_blades} blade(s) in {} spatial/PVS patch(es), {:.1} MiB compact CPU instances ({} bytes/blade); worker CPU {:.1} ms, wall-to-ready {:.1} ms",
            grass_patches.len(),
            grass_cpu_bytes as f64 / (1024.0 * 1024.0),
            std::mem::size_of::<GrassInstance>(),
            load_timings.grass_cpu_ms,
            load_timings.grass_ms,
        );
    }

    phase_lap(&mut load_timings, 11, &mut phase_clock);
    // GI was queued after geometry became immutable. Joining it only after grass
    // finalization allows the worker pool to overlap both CPU-heavy finishing stages.
    let voxel_probe_gi = if let Some(hit) = gi_memo_hit {
        hit
    } else if let Some(handle) = gi_job {
        let (grid, cpu_ms) = handle.join()?;
        load_timings.gi_ms = cpu_ms;
        grid
    } else if let Some((grid, gi_ms)) = gi_sync {
        load_timings.gi_ms = gi_ms;
        grid
    } else {
        None
    };
    if let Some(grid) = &voxel_probe_gi {
        devprintln!(
            1,
            "{name}: voxel/probe GI: {}x{}x{} probes, {:.1}-unit cells, {} occupied surface voxel(s)",
            grid.bounds[0], grid.bounds[1], grid.bounds[2], grid.cell_size, grid.occupied_voxels
        );
    }

    let steam_audio_result = if let Some(handle) = steam_audio_job {
        Some(handle.join()?)
    } else {
        steam_audio_sync
    };
    let steam_audio_acoustic_mesh = match steam_audio_result {
        Some((Ok(mesh), cpu_ms)) => {
            load_timings.steam_audio_ms = cpu_ms;
            devprintln!(
                1,
                "{name}: Steam Audio acoustic scene: {} vertices, {} triangles, {:.1} ms CPU",
                mesh.vertices.len(),
                mesh.triangles.len(),
                cpu_ms,
            );
            Some(mesh)
        }
        Some((Err(error), cpu_ms)) => {
            load_timings.steam_audio_ms = cpu_ms;
            warnings.push(format!(
                "{name}: Steam Audio acoustic scene unavailable: {error}"
            ));
            None
        }
        None => None,
    };

    let mut steam_audio_bake = None;
    let mut steam_audio_bake_request = None;
    if let (Some(acoustic_mesh), Some(cache)) = (&steam_audio_acoustic_mesh, steam_audio_bake_cache)
    {
        match crate::steam_audio::load_cached_bake(&cache) {
            Ok(Some(data)) => {
                steam_audio_bake = Some(Arc::new(data));
            }
            Ok(None) => {
                let num_threads = jobs
                    .map(|jobs| jobs.worker_count().saturating_sub(1).max(1))
                    .unwrap_or_else(|| {
                        std::thread::available_parallelism()
                            .map_or(1, |count| count.get().saturating_sub(2).clamp(1, 8))
                    });
                devprintln!(
                    1,
                    "{name}: Steam Audio bake cache MISS; background bake queued after map preparation ({num_threads} Steam Audio thread(s))"
                );
                steam_audio_bake_request = Some(crate::steam_audio::SteamAudioBakeRequest {
                    mesh: Arc::clone(acoustic_mesh),
                    cache,
                    num_threads,
                });
            }
            Err(error) => {
                // A corrupt/stale cache is rebuildable and must not block the map.
                let num_threads = jobs
                    .map(|jobs| jobs.worker_count().saturating_sub(1).max(1))
                    .unwrap_or(1);
                warnings.push(format!(
                    "{name}: Steam Audio cache invalid; rebuilding in background: {error}"
                ));
                steam_audio_bake_request = Some(crate::steam_audio::SteamAudioBakeRequest {
                    mesh: Arc::clone(acoustic_mesh),
                    cache,
                    num_threads,
                });
            }
        }
    }

    phase_lap(&mut load_timings, 12, &mut phase_clock);
    let (footprint_mark_textures, footprint_mark_blend_modes) =
        load_legacy_footprint_marks(&library, &mut assets, &mut textures);

    let texture_stats = textures.load_stats;
    let texture_hashes = textures.content_hashes();
    load_timings.texture_format_images = texture_stats.format_images;
    load_timings.texture_format_decode_ms = texture_stats.format_decode_ms;
    load_timings.texture_preload_wall_ms = texture_stats.preload_wall_ms;
    load_timings.texture_read_ms = texture_stats.read_ms;
    load_timings.texture_decode_ms = texture_stats.decode_ms;
    load_timings.texture_mip_ms = texture_stats.mip_ms;
    load_timings.texture_images = texture_stats.decoded_images;
    load_timings.generated_normal_ms = texture_stats.generated_normal_ms;
    load_timings.generated_normals = texture_stats.generated_normals;
    warnings.extend(textures.warnings);
    warnings.sort();
    warnings.dedup();

    phase_lap(&mut load_timings, 13, &mut phase_clock);
    // Join the plan started after the geometry stage. `portal_plans_ms` is the
    // plan's own compute time; `portal_wait_ms` is how long the loader stalled.
    let portal_wait_started = Instant::now();
    let (vertices, batches, pvs_batches, visibility, portal_draw_plan) =
        if let Some(plan) = portal_memo_hit {
            let (vertices, batches, pvs_batches, visibility) = portal_inputs
                .take()
                .expect("plan inputs kept on a memo hit");
            (vertices, batches, pvs_batches, visibility, plan)
        } else if let Some(handle) = portal_job {
            let (vertices, batches, pvs_batches, visibility, plan, plan_ms) = handle.join()?;
            load_timings.portal_plans_ms = plan_ms;
            (vertices, batches, pvs_batches, visibility, plan)
        } else if plan_wanted {
            let (vertices, batches, pvs_batches, visibility) = portal_inputs
                .take()
                .expect("plan inputs kept without a worker pool");
            let plan_stage = Instant::now();
            let plan = build_prepared_portal_draw_plan(
                &batches,
                &pvs_batches,
                visibility
                    .as_ref()
                    .expect("visibility checked before AUTO 4 build"),
                &vertices,
                |_| {},
            );
            load_timings.portal_plans_ms = plan_stage.elapsed().as_secs_f64() * 1000.0;
            (vertices, batches, pvs_batches, visibility, plan)
        } else {
            let (vertices, batches, pvs_batches, visibility) = portal_inputs
                .take()
                .expect("plan inputs kept when no plan is wanted");
            (
                vertices,
                batches,
                pvs_batches,
                visibility,
                PreparedPortalDrawPlan::default(),
            )
        };
    let portal_wait_ms = portal_wait_started.elapsed().as_secs_f64() * 1000.0;
    phase_lap(&mut load_timings, 14, &mut phase_clock);
    if !reused.is_empty() {
        devprintln!(
            2,
            "{name}: re-prepare reused unchanged stage(s): {}",
            reused.join(", ")
        );
    }
    if !portal_draw_plan.plan_by_cluster.is_empty() {
        devprintln!(
            2,
            "{name}: AUTO 4 map-worker plans: {} FULL piece(s), {} collapsed recipe(s) over {} physical geometr(ies) ({} already contiguous, {:.2} MiB to build lazily), {} unique plan(s) for {} cluster(s); {} recipe reuse hit(s), {} whole-plan reuse hit(s), {:.1} ms (loader waited {:.1} ms)",
            pvs_batches.len(),
            portal_draw_plan.variants.len(),
            portal_draw_plan.geometries.len(),
            portal_draw_plan.geometries.iter().filter(|geometry| geometry.contiguous).count(),
            portal_draw_plan.packed_index_count as f64 * 4.0 / (1024.0 * 1024.0),
            portal_draw_plan.plans.len(),
            portal_draw_plan.plan_by_cluster.len(),
            portal_draw_plan.reused_variant_hits,
            portal_draw_plan.reused_plan_hits,
            load_timings.portal_plans_ms,
            portal_wait_ms,
        );
    }
    let debug_volumes = match bsp.debug_volumes() {
        Ok(volumes) => Arc::new(volumes),
        Err(error) => {
            warnings.push(format!(
                "{name}: trigger/clip debug volumes unavailable: {error}"
            ));
            Arc::new(jka_assets::bsp::DebugVolumes::default())
        }
    };
    phase_lap(&mut load_timings, 15, &mut phase_clock);
    let archive_opens_after = jka_assets::pk3::archive_open_stats();
    load_timings.archive_opens = (archive_opens_after.0 - archive_opens_before.0) as u32;
    load_timings.archive_open_ms = archive_opens_after.1 - archive_opens_before.1;
    load_timings.prepare_wall_ms = prepare_started.elapsed().as_secs_f64() * 1000.0;

    Ok(PreparedMap {
        debug_volumes,
        authored_oceans,
        movement,
        collision,
        mark_surfaces: Some(mark_surfaces),
        static_models,
        physics_collision,
        weather_occlusion,
        vertices,
        legacy_dlight_triangle_surfaces,
        legacy_dlight_surfaces,
        batches,
        pvs_batches,
        portal_draw_plan,
        inline_vertices,
        inline_batches,
        inline_models,
        videos: textures.videos,
        textures: textures.images,
        footprint_mark_textures,
        footprint_mark_blend_modes,
        lightmaps,
        deluxemaps,
        static_bsp_ao_cache,
        steam_audio_acoustic_mesh,
        steam_audio_bake,
        steam_audio_bake_request,
        visibility,
        lights,
        source_map_lighting: SourceMapLighting::default(),
        sun,
        static_light_grid,
        voxel_probe_gi,
        reflection_probes,
        grass_patches,
        grass_local_fogs,
        surface_sprite_effects,
        global_fog,
        warnings,
        spawns,
        fx_runners,
        brush_entities,
        entity_graph,
        distance_cull,
        sky_portal,
        triangles,
        lightmap_pages,
        source: asset_source,
        map_file_stats: None,
        bsp_stats: Some(bsp_stats),
        load_timings,
        material_debug,
        map_hash,
        stage_keys: PrepStageKeys {
            grass: grass_key,
            gi: gi_key,
            portal: portal_key,
        },
        texture_hashes,
    })
}
