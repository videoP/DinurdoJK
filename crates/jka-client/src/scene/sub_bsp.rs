//! Server-advertised misc_bsp instances share the main map's inline handle table.
use super::{
    Arc, AssetSearchPath, DrawBatch, InlineModelGeometry, MapJobPool, MapPrepareOptions, MapSource,
    Path, PreparedMap, MAX_FILE_BYTES,
};
use std::collections::BTreeMap;

const CS_BSP_MODELS: u16 = 1612;
const MAX_SUB_BSP: u16 = 32;

pub(crate) fn server_bsp_names(
    configstrings: &BTreeMap<u16, Vec<u8>>,
) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    // Slot zero is reserved. OpenJK/jaPRO stop at the first empty slot.
    for index in 1..MAX_SUB_BSP {
        let Some(value) = configstrings
            .get(&(CS_BSP_MODELS + index))
            .filter(|value| !value.is_empty())
        else {
            break;
        };
        let name = std::str::from_utf8(value).map_err(|_| "Server sub-BSP name is not UTF-8")?;
        let name = name.replace('\\', "/").to_ascii_lowercase();
        // G_SetupBSPInstance advertises #mapname; CL_CM_LoadMap expands it to
        // maps/mapname.bsp. Also accept an already-expanded engine qpath.
        let stem = if let Some(stem) = name.strip_prefix('#') {
            let stem = stem.strip_prefix("maps/").unwrap_or(stem);
            stem.strip_suffix(".bsp").unwrap_or(stem)
        } else {
            name.strip_prefix("maps/")
                .and_then(|name| name.strip_suffix(".bsp"))
                .ok_or_else(|| format!("Invalid server sub-BSP qpath: {name}"))?
        };
        super::source::validate_map_name(stem)?;
        let name = format!("maps/{stem}.bsp");
        if name.len() >= 64 {
            return Err("Server sub-BSP name exceeds MAX_QPATH".into());
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

pub(crate) fn sub_bsp_configstring_command(command: &[u8]) -> bool {
    let mut words = command
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|word| !word.is_empty());
    if !matches!(words.next(), Some(b"cs" | b"bcs2")) {
        return false;
    }
    let index = words
        .next()
        .and_then(|word| std::str::from_utf8(word).ok())
        .and_then(|word| word.parse::<u16>().ok());
    index.is_some_and(|index| index > CS_BSP_MODELS && index < CS_BSP_MODELS + MAX_SUB_BSP)
}

pub(crate) fn prepare_source_with_sub_bsps(
    root: &Path,
    game: Option<&Path>,
    source: &MapSource,
    jobs: &MapJobPool,
    options: MapPrepareOptions,
    seed: Option<&Arc<PreparedMap>>,
    names: &[String],
) -> Result<PreparedMap, String> {
    let started = std::time::Instant::now();
    // An aggregate preparation must never lend appended textures/geometry or
    // collision to the main-map-only pipeline on a later join/restart.
    let seed = seed.filter(|map| map.sub_bsp_names.is_empty());
    let mut map = super::prepare_source_with_jobs(root, game, source, jobs, options, seed)?;
    if names.is_empty() {
        return Ok(map);
    }
    let MapSource::Bsp(main_name) = source else {
        return Err("Sub-BSPs require a compiled main map".into());
    };
    let mut assets = AssetSearchPath::open_game(root, game).map_err(|error| error.to_string())?;
    assets.set_allow_asset_overrides(options.allow_asset_overrides);
    let main_path = super::source::map_asset_name(main_name, "bsp")?;
    let main = read_bsp(&mut assets, &main_path)?;
    let imports = names
        .iter()
        .map(|name| read_bsp(&mut assets, name))
        .collect::<Result<Vec<_>, _>>()?;
    let sub_bsps: Vec<_> = names
        .iter()
        .zip(&imports)
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    map.collision = Some(jka_movement::CollisionWorld::from_bsp_with_sub_bsps(
        &main, &sub_bsps,
    )?);
    let mut imported_options = options;
    imported_options.grass = false;
    imported_options.voxel_probe_gi = false;
    imported_options.ocean = false;
    imported_options.client_physics = false;
    imported_options.steam_audio = false;
    imported_options.planar_reflections = false;
    imported_options.planar_environment = false;
    for name in names {
        let stem = &name[5..name.len() - 4];
        // Prepare each BSP separately so its external lightmaps, shader providers,
        // atlas UVs and PBR companions retain their own map/asset context.
        let child = super::prepare_with_options(root, game, stem, imported_options)
            .map_err(|error| format!("Sub-BSP {name}: {error}"))?;
        append_sub_bsp(&mut map, child)?;
    }
    map.sub_bsp_names = names.to_vec();
    map.load_timings.prepare_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(map)
}

fn read_bsp(assets: &mut AssetSearchPath, name: &str) -> Result<Vec<u8>, String> {
    assets
        .read(name, MAX_FILE_BYTES)
        .map_err(|error| error.to_string())?
        .map(|asset| asset.bytes)
        .ok_or_else(|| format!("Server BSP {name} not found on the game/base asset search path"))
}

fn append_sub_bsp(map: &mut PreparedMap, mut child: PreparedMap) -> Result<(), String> {
    let stats = map
        .bsp_stats
        .as_mut()
        .ok_or("Main map has no BSP model metadata")?;
    let child_stats = child
        .bsp_stats
        .take()
        .ok_or("Imported map has no BSP model metadata")?;
    let model_offset = u32::try_from(stats.inline_model_bounds.len())
        .map_err(|_| "BSP model count exceeds u32")?;
    let mut bounds = stats.inline_model_bounds.to_vec();
    bounds.extend_from_slice(&child_stats.inline_model_bounds);
    stats.inline_model_bounds = bounds.into();
    let mut midpoints = stats.inline_model_midpoints.to_vec();
    midpoints.extend_from_slice(&child_stats.inline_model_midpoints);
    stats.inline_model_midpoints = midpoints.into();

    let texture_offset = map.textures.len();
    let lightmap_offset = map.lightmaps.len();
    let debug_offset = u32::try_from(map.material_debug.entries.len())
        .map_err(|_| "Material count exceeds u32")?;
    Arc::make_mut(&mut map.textures).append(Arc::make_mut(&mut child.textures));
    map.texture_hashes.append(&mut child.texture_hashes);
    Arc::make_mut(&mut map.lightmaps).append(Arc::make_mut(&mut child.lightmaps));
    Arc::make_mut(&mut map.deluxemaps).append(Arc::make_mut(&mut child.deluxemaps));
    for video in &mut child.videos {
        video.texture += texture_offset;
    }
    map.videos.append(&mut child.videos);
    map.material_debug
        .entries
        .append(&mut child.material_debug.entries);

    // Imported model zero is drawn only through its server ET_MOVER instance.
    // It must not enter static world PVS, AO, GI, weather or collision geometry.
    let vertex_offset =
        u32::try_from(map.inline_vertices.len()).map_err(|_| "Inline vertices exceed u32")?;
    let vertex_end = vertex_offset
        .checked_add(u32::try_from(child.vertices.len()).map_err(|_| "Inline vertices exceed u32")?)
        .ok_or("Inline vertices exceed u32")?;
    let batch_offset = map.inline_batches.len();
    map.inline_patch_draws.extend(child.patch_draws.into_iter().map(|mut patch| {
        patch.vertices.start += vertex_offset; patch.vertices.end += vertex_offset; patch
    }));
    map.inline_vertices.append(&mut child.vertices);
    for batch in &mut child.batches {
        remap_batch(
            batch,
            vertex_offset,
            texture_offset,
            lightmap_offset,
            debug_offset,
        )?;
    }
    map.inline_batches.append(&mut child.batches);
    if map.inline_batches.len() != batch_offset {
        map.inline_models.push(InlineModelGeometry {
            model: model_offset,
            vertices: vertex_offset..vertex_end,
            batches: batch_offset..map.inline_batches.len(),
        });
    }
    let vertex_offset =
        u32::try_from(map.inline_vertices.len()).map_err(|_| "Inline vertices exceed u32")?;
    let _ = vertex_offset
        .checked_add(
            u32::try_from(child.inline_vertices.len()).map_err(|_| "Inline vertices exceed u32")?,
        )
        .ok_or("Inline vertices exceed u32")?;
    let batch_offset = map.inline_batches.len();
    map.inline_vertices.append(&mut child.inline_vertices);
    for batch in &mut child.inline_batches {
        remap_batch(
            batch,
            vertex_offset,
            texture_offset,
            lightmap_offset,
            debug_offset,
        )?;
    }
    map.inline_patch_draws.extend(child.inline_patch_draws.into_iter().map(|mut patch| {
        patch.vertices.start += vertex_offset; patch.vertices.end += vertex_offset; patch
    }));
    map.inline_batches.append(&mut child.inline_batches);
    for mut model in child.inline_models {
        model.model += model_offset;
        model.vertices.start += vertex_offset;
        model.vertices.end += vertex_offset;
        model.batches.start += batch_offset;
        model.batches.end += batch_offset;
        map.inline_models.push(model);
    }
    map.warnings.extend(child.warnings);
    Ok(())
}

fn remap_batch(
    batch: &mut DrawBatch,
    vertex_offset: u32,
    texture_offset: usize,
    lightmap_offset: usize,
    debug_offset: u32,
) -> Result<(), String> {
    batch.vertices.start = batch
        .vertices
        .start
        .checked_add(vertex_offset)
        .ok_or("Inline vertices exceed u32")?;
    batch.vertices.end = batch
        .vertices
        .end
        .checked_add(vertex_offset)
        .ok_or("Inline vertices exceed u32")?;
    for index in [
        &mut batch.texture,
        &mut batch.normal_texture,
        &mut batch.roughness_texture,
        &mut batch.height_texture,
        &mut batch.metallic_texture,
        &mut batch.specular_texture,
        &mut batch.emissive_texture,
    ] {
        if let Some(index) = index {
            *index += texture_offset;
        }
    }
    if let Some(skybox) = &mut batch.skybox {
        for index in skybox {
            *index += texture_offset;
        }
    }
    if let Some(index) = &mut batch.lightmap {
        *index += lightmap_offset;
    }
    if batch.material_debug_index != u32::MAX {
        batch.material_debug_index += debug_offset;
    }
    batch.bsp_shader_index = u32::MAX; // The main BSP's inspector table has different indices.
    batch.pvs_signature.clear();
    batch.area_signature = [0; 4];
    batch.planar_reflection = false;
    batch.planar_environment_candidate = false;
    batch.water_primary = false;
    batch.authored_ocean = None;
    batch.reflection_probe = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn misc_bsp_configstrings_follow_server_order_and_validate_qpaths() {
        let mut strings = BTreeMap::from([
            (CS_BSP_MODELS, b"reserved".to_vec()),
            (CS_BSP_MODELS + 1, b"#room".to_vec()),
            (CS_BSP_MODELS + 2, b"MAPS\\ROOM.BSP".to_vec()),
            (CS_BSP_MODELS + 3, b"#mp/platform".to_vec()),
            (CS_BSP_MODELS + 5, b"maps/after_gap.bsp".to_vec()),
        ]);
        assert_eq!(
            server_bsp_names(&strings).unwrap(),
            ["maps/room.bsp", "maps/mp/platform.bsp"]
        );
        strings.insert(CS_BSP_MODELS + 2, b"maps/../outside.bsp".to_vec());
        assert!(server_bsp_names(&strings).is_err());
        strings.insert(CS_BSP_MODELS + 2, b"#../outside".to_vec());
        assert!(server_bsp_names(&strings).is_err());
        strings.insert(CS_BSP_MODELS + 2, b"C:/outside.bsp".to_vec());
        assert!(server_bsp_names(&strings).is_err());
        strings.insert(CS_BSP_MODELS + 2, b"maps/room.bsp\0".to_vec());
        assert!(server_bsp_names(&strings).is_err());
    }
    #[test]
    fn misc_bsp_configstring_updates_include_completed_big_strings() {
        assert!(sub_bsp_configstring_command(b"cs 1613 \"maps/room.bsp\""));
        assert!(sub_bsp_configstring_command(b"bcs2 1643 \"bsp\""));
        assert!(!sub_bsp_configstring_command(b"bcs0 1613 \"maps/room.\""));
        assert!(!sub_bsp_configstring_command(b"cs 1612 \"reserved\""));
        assert!(!sub_bsp_configstring_command(b"cs 1644 \"other\""));
    }
    #[allow(dead_code)]
    mod support {
        include!("../../../jka-movement/tests/support/mod.rs");
    }

    fn render_bsp(model_count: usize, shader: &str, contents: i32) -> Vec<u8> {
        support::rewrite_lumps(
            &support::Map::default()
                .box_brush([-8.0, -16.0, -24.0], [8.0, 16.0, 24.0], contents)
                .bytes(),
            |lumps| {
                lumps[1][..64].fill(0);
                lumps[1][..shader.len()].copy_from_slice(shader.as_bytes());
                for pos in [[0.0_f32, 0.0, 0.0], [8.0, 0.0, 0.0], [0.0, 16.0, 0.0]] {
                    let mut vertex = vec![0; 80];
                    for (axis, value) in pos.iter().enumerate() {
                        vertex[axis * 4..axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
                    }
                    vertex[60..64].copy_from_slice(&1.0_f32.to_le_bytes());
                    vertex[64..].fill(255);
                    lumps[10].extend(vertex);
                }
                for index in [0_i32, 1, 2] {
                    lumps[11].extend(index.to_le_bytes());
                }
                let mut surface = vec![0; 148];
                surface[4..8].copy_from_slice(&(-1_i32).to_le_bytes());
                surface[8..12].copy_from_slice(&1_i32.to_le_bytes());
                surface[16..20].copy_from_slice(&3_i32.to_le_bytes());
                surface[24..28].copy_from_slice(&3_i32.to_le_bytes());
                surface[28..36].fill(255);
                surface[28] = 0;
                for slot in 1..4 {
                    surface[36 + slot * 4..40 + slot * 4].copy_from_slice(&(-1_i32).to_le_bytes());
                }
                lumps[13] = surface;
                lumps[14] = vec![128; 128 * 128 * 3];
                lumps[5].extend(0_i32.to_le_bytes());
                lumps[4][36..40].copy_from_slice(&1_i32.to_le_bytes());
                lumps[7][28..32].copy_from_slice(&1_i32.to_le_bytes());
                let model = lumps[7].clone();
                for _ in 1..model_count {
                    lumps[7].extend_from_slice(&model);
                }
            },
        )
    }

    #[test]
    fn menu_scenery_skips_gameplay_work_and_restores_collision_for_play() {
        let root = std::env::temp_dir().join(format!(
            "dinurdo-menu-scenery-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(root.join("maps")).unwrap();
        std::fs::write(root.join("maps/menu.bsp"), render_bsp(1, "textures/menu", 1)).unwrap();
        let options = MapPrepareOptions {
            visual_only: true,
            grass: false,
            voxel_probe_gi: false,
            ocean: false,
            pbr_materials: false,
            planar_reflections: false,
            planar_environment: false,
            ..Default::default()
        };
        let collision_jobs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = collision_jobs.clone();
        let jobs = MapJobPool::new_with_progress(move |progress| {
            if progress.task == crate::thread_activity::Task::MapCollision {
                observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }).unwrap();
        for worker_pool in [false, true] {
            let prepare = |options, seed: Option<&Arc<PreparedMap>>| {
                if worker_pool {
                    super::super::prepare_with_jobs_options(&root, None, "menu", &jobs, options, seed)
                } else if let Some(seed) = seed {
                    super::super::prepare_with_seed(&root, None, "menu", options, seed)
                } else {
                    super::super::prepare_with_options(&root, None, "menu", options)
                }.unwrap()
            };
            let before = collision_jobs.load(std::sync::atomic::Ordering::Relaxed);
            let menu = Arc::new(prepare(options, None));
            assert!(!menu.vertices.is_empty(), "menu retains its render geometry");
            assert!(menu.collision.is_none());
            assert!(menu.movement.is_none());
            assert!(menu.mark_surfaces.is_none());
            assert!(menu.physics_collision.triangle_data().is_none());
            assert!(menu.steam_audio_acoustic_mesh.is_none());
            assert_eq!(menu.load_timings.collision_ms, 0.0);
            assert_eq!(collision_jobs.load(std::sync::atomic::Ordering::Relaxed), before,
                "menu must not queue the native collision/CM optimization job");

            let gameplay = Arc::new(prepare(MapPrepareOptions { visual_only: false, ..options }, Some(&menu)));
            assert!(gameplay.collision.is_some(), "entering play must build collision even from a menu seed");
            assert!(gameplay.mark_surfaces.is_some());
            assert_eq!(gameplay.vertices.len(), menu.vertices.len());
            assert_eq!(gameplay.batches.len(), menu.batches.len());

            let before = collision_jobs.load(std::sync::atomic::Ordering::Relaxed);
            let menu_again = prepare(options, Some(&gameplay));
            assert!(menu_again.collision.is_none(), "menu must not inherit gameplay collision");
            assert!(menu_again.mark_surfaces.is_none());
            assert_eq!(collision_jobs.load(std::sync::atomic::Ordering::Relaxed), before);
        }
        assert!(root.starts_with(std::env::temp_dir()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn misc_bsp_preparation_keeps_render_collision_and_cache_handles_aligned() {
        use jka_movement::{EntityClip, TraceQuery, ENTITY_NONE};
        let root = std::env::temp_dir().join(format!(
            "dinurdo-misc-bsp-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("maps")).unwrap();
        std::fs::create_dir_all(root.join("textures")).unwrap();
        std::fs::write(
            root.join("maps/main.bsp"),
            render_bsp(9, "textures/main", 1),
        )
        .unwrap();
        std::fs::write(
            root.join("maps/child.bsp"),
            render_bsp(2, "textures/child", 4),
        )
        .unwrap();
        std::fs::write(
            root.join("maps/second.bsp"),
            render_bsp(1, "textures/child", 1),
        )
        .unwrap();
        let mut tga = vec![0; 18];
        tga[2] = 2;
        tga[12] = 1;
        tga[14] = 1;
        tga[16] = 24;
        tga.extend([0, 0, 255]);
        std::fs::write(root.join("textures/main.tga"), &tga).unwrap();
        std::fs::write(root.join("textures/child.tga"), &tga).unwrap();
        let jobs = MapJobPool::new_with_progress(|_| {}).unwrap();
        let options = MapPrepareOptions {
            grass: false,
            voxel_probe_gi: false,
            ocean: false,
            pbr_materials: false,
            planar_reflections: false,
            planar_environment: false,
            ..Default::default()
        };
        let source = MapSource::Bsp("main".into());
        let names = server_bsp_names(&BTreeMap::from([
            (CS_BSP_MODELS + 1, b"#child".to_vec()),
            (CS_BSP_MODELS + 2, b"#second".to_vec()),
        ]))
        .unwrap();
        let baseline = Arc::new(
            prepare_source_with_sub_bsps(&root, None, &source, &jobs, options, None, &[]).unwrap(),
        );
        let mut combined = prepare_source_with_sub_bsps(
            &root,
            None,
            &source,
            &jobs,
            options,
            Some(&baseline),
            &names,
        )
        .unwrap();
        assert_eq!(combined.sub_bsp_names, names);
        assert_eq!(combined.vertices.len(), baseline.vertices.len());
        assert_eq!(combined.batches.len(), baseline.batches.len());
        assert_eq!(
            combined
                .bsp_stats
                .as_ref()
                .unwrap()
                .inline_model_bounds
                .len(),
            12
        );
        for model_index in [9, 10, 11] {
            let model = combined
                .inline_models
                .iter()
                .find(|model| model.model == model_index)
                .expect("imported model render geometry");
            assert!(!model.vertices.is_empty());
            for batch in &combined.inline_batches[model.batches.clone()] {
                assert!(
                    batch.vertices.start >= model.vertices.start
                        && batch.vertices.end <= model.vertices.end
                );
                let texture = batch.texture.unwrap();
                assert!(combined.textures[texture].label.contains("child"));
                assert!(batch.lightmap.unwrap() >= baseline.lightmaps.len());
                assert!(batch.lightmap.unwrap() < combined.lightmaps.len());
                assert!(batch.pvs_signature.is_empty());
            }
        }
        let origin = [200.0, 0.0, 100.0];
        let query = TraceQuery {
            start: [250.0, 0.0, 100.0],
            end: origin,
            mins: [0.0; 3],
            maxs: [0.0; 3],
            pass_entity: ENTITY_NONE,
            mask: 4,
        };
        let world = combined.collision.as_mut().unwrap();
        assert!(
            world
                .trace_entity(
                    query,
                    EntityClip::InlineModel {
                        index: 9,
                        origin,
                        angles: [0.0; 3]
                    }
                )
                .fraction
                < 1.0
        );
        assert_eq!(world.inline_model_contents(origin, 9, origin, [0.0; 3]), 4);
        // Two misc_bsp entities may share handle 9 (as in ahfucker.dm_26).
        // Render copies need independent regions; collision remains one handle.
        let original_vertex_count = combined.inline_vertices.len();
        let collision_models = combined.bsp_stats.as_ref().unwrap().inline_model_bounds.len();
        let original = combined.inline_models.iter().find(|model| model.model == 9).unwrap().clone();
        let counts = BTreeMap::from([(9, 2), (10, 3), (999, 2)]);
        assert!(combined.reserve_inline_instances(&counts));
        assert_eq!(combined.inline_instance_capacity().get(&9), Some(&2));
        assert_eq!(combined.inline_instance_capacity().get(&10), Some(&3));
        assert!(!combined.inline_instance_capacity().contains_key(&999));
        assert_eq!(combined.bsp_stats.as_ref().unwrap().inline_model_bounds.len(), collision_models);
        let copy = combined.inline_models.iter().rev().find(|model| model.model == 9).unwrap();
        assert!(copy.vertices.start >= original.vertices.end);
        assert!(copy.batches.start >= original.batches.end);
        assert_eq!(copy.vertices.len(), original.vertices.len());
        for (a, b) in combined.inline_batches[original.batches.clone()].iter().zip(&combined.inline_batches[copy.batches.clone()]) {
            assert_eq!(a.texture, b.texture);
            assert_eq!(a.lightmap, b.lightmap);
            assert_eq!(a.vertices.start - original.vertices.start, b.vertices.start - copy.vertices.start);
            assert_eq!(a.vertices.end - original.vertices.start, b.vertices.end - copy.vertices.start);
        }
        let lengths = (combined.inline_vertices.len(), combined.inline_batches.len());
        assert!(!combined.reserve_inline_instances(&counts));
        assert!(!combined.reserve_inline_instances(&BTreeMap::from([(9, 1)])));
        assert_eq!((combined.inline_vertices.len(), combined.inline_batches.len()), lengths);
        let combined = Arc::new(combined);
        let repeated = prepare_source_with_sub_bsps(
            &root,
            None,
            &source,
            &jobs,
            options,
            Some(&combined),
            &names,
        )
        .unwrap();
        assert_eq!(
            repeated
                .bsp_stats
                .as_ref()
                .unwrap()
                .inline_model_bounds
                .len(),
            12
        );
        assert_eq!(
            repeated.inline_vertices.len(),
            original_vertex_count
        );
        let mut solo = prepare_source_with_sub_bsps(
            &root,
            None,
            &source,
            &jobs,
            options,
            Some(&combined),
            &[],
        )
        .unwrap();
        assert!(solo.sub_bsp_names.is_empty());
        assert_eq!(
            solo.bsp_stats.as_ref().unwrap().inline_model_bounds.len(),
            9
        );
        assert_eq!(
            solo.collision
                .as_mut()
                .unwrap()
                .inline_model_contents(origin, 9, origin, [0.0; 3]),
            0
        );
        assert!(prepare_source_with_sub_bsps(
            &root,
            None,
            &source,
            &jobs,
            options,
            None,
            &["maps/missing.bsp".into()]
        )
        .err()
        .unwrap()
        .contains("maps/missing.bsp"));
        drop(jobs);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
