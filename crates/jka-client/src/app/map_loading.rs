//! Map loading.
use crate::app::{
    mpsc, scene, thread, App, ApplyLatchedScope, Arc, Duration, EventLoopProxy, ExternalMapChange,
    Instant, MapLoadPurpose, MapLoadingBar, MapLoadingUi, Path, PathBuf, PhysicsMapMesh,
    RenderCommand, Sender, UserEvent, FRONTEND_BACKGROUND_MAP,
};

pub(in crate::app) struct MapLoadRequest {
    pub(in crate::app) request_id: u64,
    pub(in crate::app) started: Instant,
    pub(in crate::app) game: Option<PathBuf>,
    pub(in crate::app) source: scene::MapSource,
    pub(in crate::app) sub_bsp_names: Vec<String>,
    pub(in crate::app) prepare_options: scene::MapPrepareOptions,
    /// Gameplay/live loads build the immutable EFX light-promotion catalog.
    /// Frontend scenery and source-editor preview rebuilds skip it.
    pub(in crate::app) analyze_fx_light_profiles: bool,
    /// Gameplay/frontend/editor requests share `latest_request_id`, so the
    /// loader may skip or abandon one once a newer id is published. Studio
    /// requests carry their own ids and must always run.
    pub(in crate::app) supersedable: bool,
    /// The already-prepared copy of this same map (the restart cache), lent so
    /// the loader can copy unchanged stages instead of recomputing them.
    pub(in crate::app) seed: Option<Arc<scene::PreparedMap>>,
}

#[derive(Clone)]
pub(in crate::app) struct PreparedMapCache {
    pub(in crate::app) label: String,
    pub(in crate::app) prepare_options: scene::MapPrepareOptions,
    pub(in crate::app) map: Arc<scene::PreparedMap>,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct MapLoadingBarState {
    pub(in crate::app) task: crate::thread_activity::Task,
    pub(in crate::app) label: &'static str,
    pub(in crate::app) completed: u32,
    pub(in crate::app) total: u32,
    pub(in crate::app) skipped: bool,
}

#[derive(Debug, Clone)]
/// `perfsample <seconds> [label]`: render-thread stats windows collected
/// while the console buffer is held.
pub(in crate::app) struct PerfSample {
    pub(in crate::app) label: String,
    pub(in crate::app) started: Instant,
    pub(in crate::app) duration: Duration,
    pub(in crate::app) skipped_first: bool,
    pub(in crate::app) fps: Vec<f64>,
    pub(in crate::app) frame_ms: Vec<f64>,
    pub(in crate::app) gpu_ms: Vec<f64>,
    /// Last window's main world program binds: specialized, general.
    pub(in crate::app) world_program_binds: (u32, u32),
    pub(in crate::app) world_shader_programs: crate::runtime::WorldShaderPrograms,
    /// `waitshadermode`: hold until this r_fragmentShaderMode is requested and
    /// active with no variant compiling; `duration` is then the timeout.
    pub(in crate::app) ready_mode: Option<u8>,
}

pub(in crate::app) struct MapLoadingState {
    pub(in crate::app) request_id: u64,
    pub(in crate::app) name: String,
    pub(in crate::app) sub_bsp_names: Vec<String>,
    pub(in crate::app) active_game_dir: Option<String>,
    pub(in crate::app) preparation_finished: bool,
    pub(in crate::app) bars: [MapLoadingBarState; 12],
}

impl MapLoadingState {
    pub(in crate::app) fn new(request_id: u64, name: String, active_game: Option<&Path>) -> Self {
        use crate::thread_activity::Task;
        Self {
            request_id,
            name,
            sub_bsp_names: Vec::new(),
            active_game_dir: active_game.map(|path| path.display().to_string()),
            preparation_finished: false,
            bars: [
                MapLoadingBarState {
                    task: Task::MapBspParse,
                    label: "BSP PARSE",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapCollision,
                    label: "COLLISION",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapShaderParse,
                    label: "SHADERS",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapTextureDecode,
                    label: "TEXTURES",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapMaterials,
                    label: "MATERIALS",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapFxLightProfiles,
                    label: "FX LIGHT PROFILES",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapGeometry,
                    label: "WORLD GEOMETRY",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapGi,
                    label: "VOXEL GI",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapPortalPlans,
                    label: "PVS DRAW PLANS",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapGrass,
                    label: "GRASS",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapOcean,
                    label: "OCEAN MESH",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapAcoustics,
                    label: "ACOUSTICS",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
            ],
        }
    }

    /// Fix the row set from the settings this load was requested with.
    pub(in crate::app) fn apply_prepare_options(&mut self, options: &scene::MapPrepareOptions) {
        use crate::thread_activity::Task;
        for (enabled, task) in [
            (!options.visual_only, Task::MapCollision),
            (options.grass, Task::MapGrass),
            (options.voxel_probe_gi, Task::MapGi),
            (options.ocean, Task::MapOcean),
            (options.steam_audio, Task::MapAcoustics),
        ] {
            if !enabled {
                self.skip_task(task);
            }
        }
    }

    pub(in crate::app) fn skip_task(&mut self, task: crate::thread_activity::Task) {
        if let Some(bar) = self.bars.iter_mut().find(|bar| bar.task == task) {
            bar.skipped = true;
        }
    }

    pub(in crate::app) fn update_worker(
        &mut self,
        task: crate::thread_activity::Task,
        completed: u32,
        total: u32,
    ) {
        if let Some(bar) = self.bars.iter_mut().find(|bar| bar.task == task) {
            bar.completed = completed.min(total);
            bar.total = total;
            bar.skipped = false;
        }
    }

    pub(in crate::app) fn begin_upload(&mut self) {
        self.preparation_finished = true;
    }

    /// Show a stage as finished when its result came from the prepared-map cache.
    pub(in crate::app) fn mark_cached(&mut self, task: crate::thread_activity::Task) {
        if let Some(bar) = self.bars.iter_mut().find(|bar| bar.task == task) {
            bar.total = 1;
            bar.completed = 1;
            bar.skipped = false;
        }
    }

    pub(in crate::app) fn set_optional_task_presence(
        &mut self,
        task: crate::thread_activity::Task,
        present: bool,
        completed: bool,
    ) {
        let Some(bar) = self.bars.iter_mut().find(|bar| bar.task == task) else {
            return;
        };
        if !present {
            bar.completed = 0;
            bar.total = 0;
            bar.skipped = true;
            return;
        }
        bar.skipped = false;
        if bar.total == 0 {
            bar.total = 1;
            bar.completed = if completed { 1 } else { 0 };
        }
    }

    pub(in crate::app) fn progress_fraction(&self) -> f32 {
        let mut sum = 0.0f32;
        let mut count = 0u32;
        for bar in &self.bars {
            if bar.skipped {
                continue;
            }
            if bar.total == 0 {
                // The always-run stages that start late (materials, geometry,
                // PVS plans) hold the fraction back rather than letting it
                // jump backwards when the stage finally starts.
                if matches!(
                    bar.task,
                    crate::thread_activity::Task::MapPortalPlans
                        | crate::thread_activity::Task::MapMaterials
                        | crate::thread_activity::Task::MapGeometry
                        | crate::thread_activity::Task::MapFxLightProfiles
                ) {
                    count += 1;
                }
                continue;
            }
            sum += (bar.completed as f32 / bar.total as f32).clamp(0.0, 1.0);
            count += 1;
        }
        let prep = if count == 0 { 0.0 } else { sum / count as f32 };
        if self.preparation_finished {
            prep.max(0.92).min(0.98)
        } else {
            (prep * 0.9).clamp(0.0, 0.9)
        }
    }

    pub(in crate::app) fn ui(&self) -> MapLoadingUi {
        use crate::thread_activity::Task;
        MapLoadingUi {
            map_name: self.name.clone(),
            active_game_dir: self.active_game_dir.clone(),
            preparation_finished: self.preparation_finished,
            bars: self
                .bars
                .iter()
                .map(|bar| MapLoadingBar {
                    label: bar.label,
                    completed: bar.completed,
                    total: bar.total,
                    // Grass/ocean/acoustics depend on map content that is unknown
                    // until the BSP is parsed, so they have no row until they
                    // receive work. They sit last in `bars`, so when they appear
                    // they are added at the bottom and nothing above shifts.
                    // PVS draw plans always run for vis-bearing maps, starting
                    // once the geometry batches are final, so it stays listed as
                    // "WAIT" from the start (the panel drops it if it never
                    // receives work).
                    skipped: bar.skipped
                        || (bar.total == 0
                            && matches!(
                                bar.task,
                                Task::MapGrass | Task::MapOcean | Task::MapAcoustics
                            )),
                })
                .collect(),
        }
    }
}

pub(in crate::app) fn prepared_map_has_ocean_surface(map: &scene::PreparedMap) -> bool {
    map.batches.iter().any(|batch| {
        if !batch.water_primary {
            return false;
        }
        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        let Some(vertices) = map.vertices.get(start..end) else {
            return false;
        };
        let mut lowest = f32::INFINITY;
        let mut highest = f32::NEG_INFINITY;
        let mut facing = 0.0f32;
        for vertex in vertices {
            lowest = lowest.min(vertex.position[1]);
            highest = highest.max(vertex.position[1]);
            facing += vertex.normal[1];
        }
        // Keep this in lock-step with renderer::is_upward_water_face: only the
        // upward horizontal face of a water brush receives an FFT ocean mesh.
        lowest.is_finite() && facing > 0.0 && lowest >= highest - 1.0
    })
}

impl App {
    pub(in crate::app) fn current_map_prepare_options(&self) -> scene::MapPrepareOptions {
        scene::MapPrepareOptions {
            visual_only: false,
            patch_subdivisions_bits: self.video.patch_subdivisions.to_bits(),
            patch_stitching: self.video.patch_stitching,
            picmip: self.video.picmip,
            grass: self.video.grass,
            voxel_probe_gi: self.video.voxel_probe_gi,
            ocean: self.video.ocean,
            client_physics: self.video.client_physics,
            gen_normal_maps: self.video.gen_normal_maps,
            float_lightmap: self.video.float_lightmap && (self.video.hdr || self.video.hdr_output != 0),
            planar_reflections: self.video.reflection_quality.planar_slot_budget() > 0,
            planar_environment: self.video.reflection_quality.promotes_environment_planars(),
            omit_environment_stages: self.video.reflection_quality.omits_environment_stages(),
            source_spatial_batches: self.video.gpu_driven,
            pbr_materials: self.video.pbr,
            allow_asset_overrides: self.video.allow_asset_overrides,
            steam_audio: self.audio.steam_audio,
            // `menudevmap`: the stage look (water, retextures, fx) without props.
            menu_look: self.menu_look_play,
            menu_props: false,
        }
    }

    pub(in crate::app) fn request_map(&mut self, source: scene::MapSource) {
        self.leave_render_studio_for_launch();
        self.request_map_internal(source, MapLoadPurpose::Gameplay);
    }

    pub(in crate::app) fn request_studio_demo_map(&mut self, source: scene::MapSource) {
        self.request_map_internal(source, MapLoadPurpose::StudioDemo);
    }

    /// Client physics only needs the BSP's static triangle mesh, so turning it
    /// on for a map that was prepared without one builds just that mesh on a
    /// worker instead of restarting the renderer and re-preparing the world.
    pub(in crate::app) fn ensure_map_physics_mesh(&mut self) {
        if !self.video.client_physics
            || self.front_end
            || self.loading.is_some()
            || self.live_without_world
            || self.demo_without_world
        {
            return;
        }
        let label = self.initial_source.label();
        if self.physics_mesh_label.as_deref() == Some(label.as_str())
            || !self
                .prepared_map_cache
                .as_ref()
                .is_some_and(|cache| cache.label == label)
        {
            return;
        }
        let base = self.base.clone();
        let game = self.game.clone();
        let source = self.initial_source.clone();
        let allow_asset_overrides = self.video.allow_asset_overrides;
        let proxy = self.proxy.clone();
        let event_label = label.clone();
        let spawn = thread::Builder::new()
            .name("jka-physics-mesh".into())
            .spawn(move || {
                let result = scene::prepare_physics_collision(
                    &base,
                    game.as_deref(),
                    &source,
                    allow_asset_overrides,
                );
                let _ = proxy.send_event(UserEvent::PhysicsMeshReady {
                    label: event_label,
                    result,
                });
            });
        match spawn {
            Ok(_) => {
                self.physics_mesh_label = Some(label);
                self.console_status =
                    "CLIENT PHYSICS: BUILDING MAP COLLISION IN THE BACKGROUND".into();
            }
            Err(error) => {
                self.push_console_line(format!("^3CLIENT PHYSICS:^7 worker unavailable ({error})"));
            }
        }
    }

    pub(in crate::app) fn finish_physics_mesh(
        &mut self,
        label: String,
        result: Result<PhysicsMapMesh, String>,
    ) {
        // A map change or reload since the job started makes the mesh stale.
        if self.initial_source.label() != label {
            return;
        }
        let mesh = match result {
            Ok(mesh) => mesh,
            Err(error) => {
                self.physics_mesh_label = None;
                self.push_console_line(format!("^1CLIENT PHYSICS MAP COLLISION: {error}"));
                return;
            }
        };
        if let Some(cache) = self
            .prepared_map_cache
            .as_mut()
            .filter(|cache| cache.label == label)
        {
            Arc::make_mut(&mut cache.map).physics_collision = mesh.clone();
            cache.prepare_options.client_physics = true;
        }
        self.map_physics_collision = mesh;
        if let Some(session) = self.game_session.as_mut() {
            if let Err(error) = session
                .player_presenter
                .set_physics_map_mesh(&self.map_physics_collision)
            {
                eprintln!("RAPIER MAP COLLISION ERROR: {error}");
            }
        }
        self.console_status = "CLIENT PHYSICS: MAP COLLISION READY".into();
        self.publish_ui();
    }

    pub(in crate::app) fn request_map_edit_preview(&mut self, path: PathBuf, text: String) {
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        let request_id = self.latest_request_id;
        self.map_loader_latest_requested.store(request_id, std::sync::atomic::Ordering::Release);
        self.map_edit_preview_request_id = Some(request_id);
        self.map_edit_preview_pending_collision = None;
        self.map_edit_preview_dirty = false;
        let source = scene::MapSource::MapEditPreview {
            path,
            text: Arc::<str>::from(text),
        };
        let mut prepare_options = self.current_map_prepare_options();
        // Source brush movement collision is rebuilt by source-map preparation,
        // but the expensive optional Rapier and Steam Audio products are not.
        prepare_options.client_physics = false;
        prepare_options.steam_audio = false;
        self.latest_prepare_options = prepare_options;
        println!("Map editor preview: background rebuild {}", source.label());
        if self
            .loader_tx
            .send(MapLoadRequest {
                request_id,
                started: Instant::now(),
                game: self.game.clone(),
                source,
                prepare_options,
                analyze_fx_light_profiles: false,
                supersedable: true,
                seed: None,
                sub_bsp_names: Vec::new(),
            })
            .is_err()
        {
            self.map_edit_preview_request_id = None;
            if let Some(editor) = self.map_editor.as_mut() {
                editor.status = "Textured preview worker is unavailable; wireframe/solid preview remains active.".into();
            }
        }
    }

    /// Start map preparation without committing the UI to leave the front end.
    /// Used by the speculative live-server getinfo path while the authenticated
    /// connection/gamestate handshake is still in progress.
    pub(in crate::app) fn prefetch_live_map(&mut self, source: scene::MapSource) {
        self.request_map_internal(source, MapLoadPurpose::LivePrefetch);
    }

    pub(in crate::app) fn queue_map_edit_preview(&mut self) {
        if self.map_edit_preview_request_id.is_some() {
            self.map_edit_preview_dirty = true;
            return;
        }
        let preview =
            self.map_editor
                .as_ref()
                .and_then(|editor| match editor.working_source_text() {
                    Ok(text) => Some((editor.source_path.clone(), text)),
                    Err(error) => {
                        eprintln!("Map editor preview source build failed: {error}");
                        None
                    }
                });
        if let Some((path, text)) = preview {
            self.request_map_edit_preview(path, text);
        } else if let Some(editor) = self.map_editor.as_mut() {
            editor.status = "Could not build textured brush preview; see console.".into();
        }
    }

    /// Watch the one loose source `.map` backing the active source-map editor.
    /// External saves reuse the exact same coalesced background preview path as
    /// DinurdoJK brush edits, so render geometry and gameplay collision swap
    /// together without recreating the local server/player state.
    pub(in crate::app) fn tick_source_map_disk_reload(&mut self, now: Instant) {
        // `map_editor` intentionally survives until the replacement map lands so
        // pending edits are not destroyed prematurely. Do not let that stale
        // editor race a real map/menu transition with a preview request.
        if self.front_end || self.loading.is_some() {
            return;
        }
        let change = self
            .map_editor
            .as_mut()
            .map_or(ExternalMapChange::None, |editor| {
                editor.poll_external_change_with_jobs(now, Some(&self.background_jobs))
            });
        match change {
            ExternalMapChange::None => {}
            ExternalMapChange::Reloaded => {
                let path = self
                    .map_editor
                    .as_ref()
                    .map(|editor| editor.source_path.display().to_string())
                    .unwrap_or_else(|| "source map".to_owned());
                self.console_status = format!("MAP SOURCE CHANGED: REBUILDING {path}");
                self.push_console_line(format!(
                    "^5MAP LIVE RELOAD:^7 external save detected for {path}; rebuilding in background"
                ));
                self.queue_map_edit_preview();
                self.publish_snapshot();
                self.publish_ui();
                self.egui_repaint_requested = true;
            }
            ExternalMapChange::Conflict(message) => {
                self.console_status = "MAP LIVE RELOAD PAUSED: EXTERNAL/LOCAL EDIT CONFLICT".into();
                self.push_console_line(format!("^3MAP LIVE RELOAD:^7 {message}"));
                self.publish_ui();
                self.egui_repaint_requested = true;
            }
            ExternalMapChange::Error(message) => {
                self.console_status = format!("MAP LIVE RELOAD: {message}");
                self.push_console_line(format!("^3MAP LIVE RELOAD:^7 {message}"));
                self.publish_ui();
                self.egui_repaint_requested = true;
            }
        }
    }

    pub(in crate::app) fn request_frontend_background(&mut self) {
        // Back at the menu: a following gameplay load is stock again.
        self.menu_look_play = false;
        // The exported props file is part of the cache identity, so an edited
        // layout never shows a still of the previous one.
        let props_stamp = std::fs::metadata(
            self.game_write_path("mapmods").join("mp").join("duel9.ent"),
        )
        .ok()
        .map(|meta| {
            let seconds = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs());
            format!("{:x}-{:x}", meta.len(), seconds)
        })
        .unwrap_or_else(|| "0".to_owned());
        let scene_id = format!("{}-{props_stamp}", super::constants::FRONTEND_BACKGROUND_SCENE_ID);
        self.request_frontend_background_scene(&scene_id, FRONTEND_BACKGROUND_MAP);
    }

    /// Scene selection entry point: future UI/config can select any map *and*
    /// give differently staged camera/lighting treatments distinct cache ids.
    /// The snapshot belongs to the authored scene, not specifically duel3.
    pub(in crate::app) fn request_frontend_background_scene(&mut self, scene_id: &str, map: &str) {
        crate::startup_timeline::mark(crate::startup_timeline::Milestone::MapRequested);
        self.select_frontend_scene(scene_id);
        self.request_map_internal(
            scene::MapSource::Bsp(map.to_owned()),
            MapLoadPurpose::FrontendBackground,
        );
    }

    pub(in crate::app) fn frontend_background_prepare_options(&self) -> scene::MapPrepareOptions {
        let mut options = self.current_map_prepare_options();
        // The menu scene is visual-only. Do not build gameplay-only collision or
        // kick an offline Steam Audio bake simply because duel3 is on screen.
        options.visual_only = true;
        options.client_physics = false;
        options.steam_audio = false;
        options.menu_look = true;
        options.menu_props = true;
        options
    }

    /// Runtime-spawned brush entities can share a model handle. Grow once from
    /// the prepared cache when a new peak is observed; never truncate the list.
    pub(in crate::app) fn ensure_inline_instance_capacity(&mut self) {
        if self.loading.is_some() {
            return;
        }
        let Some(cache) = self.prepared_map_cache.as_ref() else {
            return;
        };
        let mut counts = std::collections::BTreeMap::new();
        let authoritative = self.game_session.as_ref()
            .filter(|session| session.phase == crate::app::SessionPhase::Playing)
            .filter(|session| session.current_snapshot.is_some());
        if let Some(session) = authoritative {
            for instance in session.inline_models.iter() {
                *counts.entry(instance.model).or_insert(0) += 1;
            }
        } else {
            // No CGame snapshot owns the movers yet. Preserve the compiled
            // representation (one world-space pose per BSP inline model).
            for model in &cache.map.inline_models {
                counts.entry(model.model).or_insert(1);
            }
        }
        // Local BMODEL stamps and the active preview share the exact
        // world-geometry pipeline. Reserve distinct render slots for each;
        // resubmission only occurs when this total exceeds past capacity.
        for placement in &self.model_placement.bmodel_placed {
            *counts.entry(placement.model).or_insert(0) += 1;
        }
        if self.model_placement.active
            && self.model_placement.kind == super::egui_model_placement::PlacementKind::Bmodel
        {
            if let Some(model) = self.model_placement.bmodel_selection {
                *counts.entry(model).or_insert(0) += 1;
            }
        }
        let capacity = cache.map.inline_instance_capacity();
        if !counts
            .iter()
            .any(|(id, count)| capacity.get(id).is_some_and(|have| count > have))
        {
            return;
        }
        let mut map = cache.map.as_ref().clone();
        map.reserve_inline_instances(&counts);
        if let Some(cache) = self.prepared_map_cache.as_mut() {
            cache.map = Arc::new(map.clone());
        }
        self.render_command(RenderCommand::GrowInlineInstances { map: Box::new(map) });
    }
    pub(in crate::app) fn active_map_prepare_options(&self) -> scene::MapPrepareOptions {
        if self.front_end && self.frontend_cinematic.is_some() {
            self.frontend_background_prepare_options()
        } else {
            self.current_map_prepare_options()
        }
    }

    pub(in crate::app) fn request_map_internal(
        &mut self,
        source: scene::MapSource,
        purpose: MapLoadPurpose,
    ) {
        if purpose != MapLoadPurpose::FrontendBackground {
            self.stop_menu_ambience();
        }
        // A real world/cgame registration must not inherit runtime redirects
        // from the previous map. Remap-only presentation uploads bypass this
        // entry point; a new gamestate replays its CS_SHADERSTATE normally.
        // A superseded early frontend result must not hold a stale BSP alive.
        self.deferred_frontend_map = None;
        self.shader_remaps.clear();
        self.shader_remap_ui = Default::default();
        self.shader_remap_upload = false;
        self.shader_remap_dirty = false;
        let sub_bsp_names = if purpose == MapLoadPurpose::FrontendBackground {
            Vec::new()
        } else {
            match self.server_sub_bsp_names_for(&source) {
                Ok(names) => names,
                Err(error) => {
                    self.push_console_line(format!("^1BSP instances: {error}"));
                    return;
                }
            }
        };
        self.map_edit_preview_request_id = None;
        self.map_edit_preview_pending_collision = None;
        self.map_edit_preview_dirty = false;
        // Never let a newly-created CGame use the previous map/fs_game's
        // inferred-light catalog while this map is still preparing. The map
        // worker publishes the replacement catalog together with MapPrepared.
        if purpose != MapLoadPurpose::FrontendBackground {
            let catalog = Arc::new(
                crate::fx::light_promotion::FxLightPromotionCatalog::default(),
            );
            self.fx_light_promotion_catalog = catalog;
        }
        if purpose != MapLoadPurpose::FrontendBackground {
            self.render_command(RenderCommand::SetAssetPreviewMode(false));
            self.render_command(RenderCommand::SetAssetPreviewViewport(None));
            self.asset_preview_viewport_key = None;
            self.clear_asset_preview_runtime();
        }
        self.apply_latched_console_cvars(ApplyLatchedScope::MapLoad);
        // PBR material selection is map-load scoped; synchronize the renderer at
        // the same boundary rather than when a latched cvar is typed. Rebuild the
        // local presentation asset cache so its VFS/material policy matches too.
        self.sync_pbr();
        self.local_player_presenter = None;
        self.pending_frontend_map_launch = self.front_end && purpose == MapLoadPurpose::Gameplay;
        self.initial_source = source.clone();
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        let request_id = self.latest_request_id;
        self.map_loader_latest_requested.store(request_id, std::sync::atomic::Ordering::Release);
        self.frontend_background_request_id =
            (purpose == MapLoadPurpose::FrontendBackground).then_some(request_id);
        if purpose != MapLoadPurpose::FrontendBackground {
            self.frontend_cinematic = None;
        }
        let label = source.label();
        self.static_ao_progress = None;
        let prepare_options = if purpose == MapLoadPurpose::FrontendBackground {
            self.frontend_background_prepare_options()
        } else {
            self.current_map_prepare_options()
        };
        self.latest_prepare_options = prepare_options;
        let mut loading = MapLoadingState::new(request_id, label.clone(), self.game.as_deref());
        loading.sub_bsp_names = sub_bsp_names.clone();
        loading.apply_prepare_options(&prepare_options);
        // Frontend scenery is speculative/non-blocking: keep the menu usable
        // while duel3 prepares, then the world simply appears behind it.
        self.loading = (purpose != MapLoadPurpose::FrontendBackground).then_some(loading);
        self.console_status = if purpose == MapLoadPurpose::FrontendBackground {
            format!("MAIN MENU - PREPARING 3D BACKGROUND {label}...")
        } else {
            format!("LOADING {label} ON MAP WORKER...")
        };
        rverbose!(1, "map load {label}: background preparation started");
        let seed = self
            .prepared_map_cache
            .as_ref()
            .filter(|cache| cache.label == label)
            .or_else(|| self.cross_game_map_seed.as_ref().filter(|cache| cache.label == label))
            .map(|cache| Arc::clone(&cache.map));
        // A cross-mod seed is only useful for the very next map preparation.
        // Its BSP hash, texture bytes and per-stage fingerprints are checked
        // on the worker before any portion is reused. External lightmaps are
        // always loaded from the new game's active asset search path.
        self.cross_game_map_seed = None;
        if self
            .loader_tx
            .send(MapLoadRequest {
                request_id,
                started: Instant::now(),
                game: self.game.clone(),
                source,
                prepare_options,
                analyze_fx_light_profiles: purpose != MapLoadPurpose::FrontendBackground,
                supersedable: true,
                seed,
                sub_bsp_names,
            })
            .is_err()
        {
            self.console_status = "MAP WORKER IS NOT AVAILABLE".into();
            self.loading = None;
        }
        self.publish_ui();
    }

    /// Re-upload the already prepared CPU map after a renderer/device restart.
    ///
    /// Backend changes invalidate every GPU resource, but they do not invalidate
    /// BSP parsing, collision, decoded textures, grass placement, probe data, or
    /// any of the other CPU preparation. Keeping one prepared copy avoids doing
    /// the full map-loader job again just because Vulkan/DX12 changed.
    pub(in crate::app) fn reupload_cached_map(&mut self) -> bool {
        let label = self.initial_source.label();
        let prepare_options = self.active_map_prepare_options();
        let clone_started = Instant::now();
        let Some(mut map) = self.prepared_map_cache.as_ref().and_then(|cache| {
            // The Rapier mesh is patched in by its own worker (and lives on the
            // App, not the renderer), so it never invalidates the cached world.
            let mut prepared = cache.prepare_options;
            prepared.client_physics = prepare_options.client_physics;
            // Any reflection quality the cached map can serve is reusable as is;
            // the effective-quality logic keeps the renderer within it.
            if Self::reflection_prep_serves(
                prepared,
                (
                    prepare_options.planar_reflections,
                    prepare_options.planar_environment,
                    prepare_options.omit_environment_stages,
                ),
            ) {
                prepared.planar_reflections = prepare_options.planar_reflections;
                prepared.planar_environment = prepare_options.planar_environment;
                prepared.omit_environment_stages = prepare_options.omit_environment_stages;
            }
            (cache.label == label && prepared == prepare_options
                && self.server_sub_bsp_names_for(&self.initial_source).is_ok_and(|names| names == cache.map.sub_bsp_names))
                .then(|| Box::new(cache.map.as_ref().clone()))
        }) else {
            return false;
        };
        let clone_ms = clone_started.elapsed().as_secs_f64() * 1000.0;

        // These timings describe CPU preparation from the original load. A cached
        // renderer restart did none of that work, so keep the restart load report
        // honest and show only its fresh GPU upload/first-frame cost.
        // A vid_restart reuses the pristine CPU cache; reconstruct the
        // configured presentation variant before handing it to the renderer.
        if !self.shader_remaps.is_empty() {
            if let Err(error) = scene::apply_shader_remaps_in_place(
                &mut map, &self.base, self.game.as_deref(), prepare_options, &self.shader_remaps,
            ) {
                self.push_console_line(format!("^1shader remap: {error}"));
            }
        }
        if self.video.stage_collapse_extra {
            scene::apply_reverse_lightmap_collapse_in_place(&mut map);
        }
        if self.video.additive_stage_fusion {
            scene::fuse_additive_stage_fusion_in_place(&mut map);
        } else {
            scene::expand_additive_stage_fusion_in_place(&mut map);
        }
        map.load_timings = scene::MapLoadTimings::default();

        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        let request_id = self.latest_request_id;
        let started = Instant::now();
        self.static_ao_progress = None;
        self.preserve_game_state_on_next_map_upload = false;
        let mut loading = MapLoadingState::new(request_id, label.clone(), self.game.as_deref());
        loading.sub_bsp_names = map.sub_bsp_names.clone();
        loading.apply_prepare_options(&prepare_options);
        let has_ocean = prepare_options.ocean && prepared_map_has_ocean_surface(map.as_ref());
        loading.set_optional_task_presence(
            crate::thread_activity::Task::MapOcean,
            has_ocean,
            false,
        );
        // The CPU preparation is the cached copy, so every stage it covers is done.
        {
            use crate::thread_activity::Task;
            let mut cached = vec![
                Task::MapBspParse,
                Task::MapCollision,
                Task::MapShaderParse,
                Task::MapTextureDecode,
                Task::MapMaterials,
                Task::MapFxLightProfiles,
                Task::MapGeometry,
            ];
            if prepare_options.grass {
                cached.push(Task::MapGrass);
            }
            if prepare_options.voxel_probe_gi {
                cached.push(Task::MapGi);
            }
            if prepare_options.steam_audio {
                cached.push(Task::MapAcoustics);
            }
            if !map.portal_draw_plan.plan_by_cluster.is_empty() {
                cached.push(Task::MapPortalPlans);
            }
            for task in cached {
                loading.mark_cached(task);
            }
        }
        loading.begin_upload();
        self.stop_menu_ambience();
        self.loading = Some(loading);
        self.console_status = format!(
            "RE-UPLOADING CACHED {label} TO {}...",
            self.video.renderer_backend.label()
        );
        rverbose!(
            1,
            "[VID_RESTART] reused prepared map cache; CPU clone {:.1} ms (skipped BSP/assets/collision/shaders/textures/grass/GI/ocean preparation)",
            clone_ms
        );
        self.publish_ui();
        self.render_command(RenderCommand::LoadMap {
            request_id,
            started,
            name: label,
            map,
            studio_world: None,
        });
        self.publish_snapshot();
        true
    }

    pub(in crate::app) fn sync_shader_remap_presenters(&mut self) {
        let remaps = self.shader_remaps.clone();
        if let Some(session) = self.game_session.as_mut() {
            session.player_presenter.set_shader_remaps(remaps.clone());
            session.entity_presenter.set_shader_remaps(remaps.clone());
            // The promotion cache is keyed by the authored FX shader name; a
            // remap can change its effective stages/texture without changing
            // that key, so invalidate alongside EntityPresenter's material cache.
            session.fx_light_promoter.invalidate_materials();
        }
        if let Some(presenter) = self.asset_preview_player_presenter.as_mut() {
            presenter.set_shader_remaps(remaps.clone());
        }
        if let Some(presenter) = self.asset_preview_entity_presenter.as_mut() {
            presenter.set_shader_remaps(remaps.clone());
        }
        if let Some(presenter) = self.local_player_presenter.as_mut() {
            presenter.set_shader_remaps(remaps);
        }
    }

    /// Cold mutation-path check: an entity/MD3/GLM-only R_RemapShader must
    /// never trigger a BSP world upload. Compare effective one-hop redirects
    /// and time offsets for *actual world batch* shader names; unused catalog
    /// entries and chained targets do not affect the world. No frame hot-path
    /// lookup, additional draw, or persistent per-frame bookkeeping.
    fn shader_remap_changes_world(&self, before: &scene::ShaderRemapState) -> bool {
        if self.loading.is_some() {
            return true; // A new map may use a shader absent from the old map.
        }
        let Some(cache) = &self.prepared_map_cache else {
            // A pending map load still needs to fold the redirect into its
            // initial upload; do not accidentally drop the dirty state.
            return true;
        };
        let map = cache.map.as_ref();
        map.batches.iter().chain(&map.pvs_batches).chain(&map.inline_batches).any(|batch| {
            let Some(entry) = map.material_debug.entries.get(batch.material_debug_index as usize) else {
                // An unknown debug index cannot be remapped by rebuild_batches.
                return false;
            };
            let source = scene::canonical_shader_name(&entry.name);
            let prior_name = before.effective_name(&source);
            let next_name = self.shader_remaps.effective_name(&source);
            prior_name != next_name
                || before.time_offset_for(&prior_name) != self.shader_remaps.time_offset_for(&next_name)
        })
    }

    fn apply_visual_shader_remap(
        &mut self,
        old: &str,
        new: &str,
        time_offset: Option<f32>,
    ) -> Result<bool, String> {
        let previous = self.shader_remaps.clone();
        if !self.shader_remaps.apply(old, new, time_offset) {
            return Ok(false);
        }
        self.sync_shader_remap_presenters();
        if self.shader_remap_changes_world(&previous) {
            self.rebuild_shader_remapped_world()?;
        } else {
            rverbose!(1, "[SHADER REMAP] model/FX-only redirect: no BSP world reload");
        }
        Ok(true)
    }

    /// TaystJK cgame-local `/remapShader <old> <new>`: direct renderer remap,
    /// intentionally independent of cg_remaps and with no timeOffset argument.
    pub(in crate::app) fn local_remap_shader(&mut self, old: &str, new: &str) {
        if let Err(error) = self.apply_visual_shader_remap(old, new, None) {
            self.push_console_line(format!("^1remapShader: {error}"));
        }
    }

    /// Display effective shader redirects (both local /remapShader and server
    /// CS_SHADERSTATE), unlike the narrower server-only TaystJK listing.
    pub(in crate::app) fn local_list_shader_remaps(&mut self) {
        // Take owned names before writing console output, which borrows App mutably.
        let remaps: Vec<(String, String)> = self.shader_remaps.redirects()
            .map(|(from, to)| (from.to_owned(), to.to_owned()))
            .collect();
        if remaps.is_empty() {
            self.push_console_line("^3listRemaps:^7 no active shader remaps".to_owned());
            return;
        }
        self.push_console_line(format!("^3listRemaps:^7 {} active remap(s)", remaps.len()));
        for (from, to) in remaps {
            self.push_console_line(format!(" ^7{from} ^3->^7 {to}"));
        }
    }

    /// Consume only shader-remap renderer actions, preserving unrelated cgame
    /// side effects for their own consumers. Policy exactly follows TaystJK:
    /// CS_SHADERSTATE uses the models/players/* exception at cg_remaps=1,
    /// while explicit server remapShader is gated only by non-zero.
    ///
    /// This only mutates logical renderer state. MapPrepared calls it before the
    /// first upload so initial CS_SHADERSTATE remaps are folded into that upload;
    /// frame-time callers request a cold-path world rebuild only when it changed.
    pub(in crate::app) fn consume_server_shader_remaps(&mut self) -> bool {
        let remaps = match self.game_session.as_mut() {
            Some(session) => session.client_game.drain_shader_remap_actions(),
            None => return false,
        };
        if remaps.is_empty() {
            return false;
        }
        let policy = self.japro_cg.remaps;
        let mut changed = false;
        for (source, remap) in remaps {
            if policy == 0 {
                continue;
            }
            if policy == 1
                && source == crate::cgame::ShaderRemapSource::ShaderState
                && scene::canonical_shader_name(&remap.from).starts_with("models/players/")
            {
                continue;
            }
            // An explicit editor OFF toggle wins over later CS_SHADERSTATE
            // refreshes. Keep the newest server target as the remembered
            // replacement, but do not reactivate the live redirect until
            // the user enables it again.
            let old = scene::canonical_shader_name(&remap.from);
            if let Some(saved) = self.shader_remap_ui.disabled.get_mut(&old) {
                *saved = scene::canonical_shader_name(&remap.to);
                continue;
            }
            changed |= self.shader_remaps.apply(
                &remap.from,
                &remap.to,
                Some(remap.time_offset_seconds()),
            );
        }
        if changed {
            self.sync_shader_remap_presenters();
        }
        changed
    }

    pub(in crate::app) fn process_server_shader_remaps(&mut self) {
        let previous = self.shader_remaps.clone();
        if !self.consume_server_shader_remaps() {
            return;
        }
        if self.shader_remap_changes_world(&previous) {
            if let Err(error) = self.rebuild_shader_remapped_world() {
                self.push_console_line(format!("^1shader remap: {error}"));
            }
        } else {
            rverbose!(1, "[SHADER REMAP] server model/FX-only redirect: no BSP world reload");
        }
    }

    /// Rebuild only the material-facing part of the prepared world for the
    /// current R_RemapShader state, then use the normal GPU upload path. This
    /// is deliberately a cold-path operation: once uploaded, the render hot
    /// path is identical to an ordinarily prepared map (no remap hash lookup,
    /// string comparison, or extra branch per draw).
    pub(in crate::app) fn rebuild_shader_remapped_world(&mut self) -> Result<bool, String> {
        // Coalesce remaps submitted before the previous replacement reaches
        // the renderer. Material replacement is not a level load.
        if self.loading.is_some() || self.shader_remap_upload {
            self.shader_remap_dirty = true;
            return Ok(false);
        }
        let Some(cache) = self.prepared_map_cache.as_ref() else {
            // The gamestate can arrive before map preparation/upload. Keep the
            // desired state and apply it when the pristine map cache exists.
            self.shader_remap_dirty = true;
            return Ok(false);
        };
        let label = cache.label.clone();
        let options = cache.prepare_options;
        let base_texture_count = cache.map.textures.len();
        let base_map = Arc::clone(&cache.map);
        let root = self.base.clone();
        let game = self.game.clone();
        let remaps = self.shader_remaps.clone();
        let reverse_lightmap = self.video.stage_collapse_extra;
        let additive_fusion = self.video.additive_stage_fusion;
        let proxy = self.proxy.clone();

        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        let request_id = self.latest_request_id;
        let started = Instant::now();
        self.shader_remap_upload = true;
        self.shader_remap_dirty = false;
        // Use the app's existing persistent jka-background-cpu lane, which
        // already handles non-frame-critical tasks. This queues no new OS
        // thread or Rayon global-pool work for a shader remap; ordinary draw
        // frames never consult the queue.
        if let Err(error) = self.background_jobs.cpu(move || {
            // The shared lane survives a panicking task. Always send a
            // completion so the app can release shader_remap_upload too.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scene::prepare_remap_plan(
                    base_map, &root, game.as_deref(), options, &remaps,
                    reverse_lightmap, additive_fusion,
                ).map(Box::new)
            })).unwrap_or_else(|_| Err("shader remap preparation worker panicked".to_owned()));
            let _ = proxy.send_event(UserEvent::ShaderRemapPrepared {
                request_id, started, name: label, base_texture_count, result,
            });
        }) {
            // A full/disconnected background queue must not strand future
            // remaps behind the upload-in-flight flag. Preserve the desired
            // logical remap for the next opportunity to submit it.
            self.shader_remap_upload = false;
            self.shader_remap_dirty = true;
            return Err(format!("could not queue shader remap on background CPU worker: {error}"));
        }
        Ok(true)
    }
}

/// Inputs the full FX promotion catalog is derived from; see `spawn_map_loader`.
#[derive(PartialEq, Eq)]
struct FxCatalogKey {
    game: Option<PathBuf>,
    allow_asset_overrides: bool,
    pbr_materials: bool,
    index: jka_assets::pk3::VfsIndexToken,
}

pub(in crate::app) fn spawn_map_loader(
    base: PathBuf,
    proxy: EventLoopProxy<UserEvent>,
    latest_requested: Arc<std::sync::atomic::AtomicU64>,
) -> Result<Sender<MapLoadRequest>, String> {
    let (tx, rx) = mpsc::channel::<MapLoadRequest>();
    thread::Builder::new()
        .name("jka-map-loader".into())
        .spawn(move || {
            let progress_proxy = proxy.clone();
            let jobs = match crate::map_jobs::MapJobPool::new_with_progress_and_latest(
                move |progress| {
                    let _ = progress_proxy.send_event(UserEvent::MapLoadProgress {
                        request_id: progress.request_id,
                        task: progress.task,
                        completed: progress.completed,
                        total: progress.total,
                    });
                },
                Arc::clone(&latest_requested),
            ) {
                Ok(jobs) => jobs,
                Err(error) => {
                    eprintln!("Map worker pool: {error}");
                    return;
                }
            };
            println!(
                "Map loader: {} worker job thread(s) available",
                jobs.worker_count()
            );
            // The full promotion catalog depends only on the VFS (game dir,
            // override policy, index identity) and the shader library parsed
            // from it, never on the map, so map changes and rejoins reuse it.
            let mut fx_catalog_cache: Option<(
                FxCatalogKey,
                Arc<crate::fx::light_promotion::FxLightPromotionCatalog>,
            )> = None;
            while let Ok(request) = rx.recv() {
                if request.supersedable
                    && request.request_id != latest_requested.load(std::sync::atomic::Ordering::Acquire)
                {
                    continue; // A newer map won before this queued request even started.
                }
                jobs.begin_request(request.request_id);
                jobs.set_supersedable(request.supersedable);
                let name = request.source.label();
                let _activity = crate::thread_activity::activity(
                    crate::thread_activity::ThreadSlot::MapLoader,
                    crate::thread_activity::Task::MapPrepare,
                );
                match scene::prepare_source_with_sub_bsps(
                    &base,
                    request.game.as_deref(),
                    &request.source,
                    &jobs,
                    request.prepare_options,
                    request.seed.as_ref(),
                    &request.sub_bsp_names,
                ) {
                    Ok(mut map) => {
                        if jobs.is_superseded() {
                            continue;
                        }
                        map.intern_images();
                        // Promotion profiling is a real map-prep stage. It runs
                        // after the world shader table is available so it can reuse
                        // that parsed data instead of reparsing every .shader file,
                        // but all EFX reads/texture decodes/image sampling happen on
                        // a map-pool worker rather than the presentation thread.
                        let menu_fx_profiles = request.prepare_options.menu_props && !map.fx_runners.is_empty();
                        let fx_light_promotion = if request.analyze_fx_light_profiles || menu_fx_profiles {
                            let profile_started = Instant::now();
                            let opened = jka_assets::pk3::AssetSearchPath::open_game(
                                &base,
                                request.game.as_deref(),
                            );
                            match opened {
                                Ok(mut fx_assets) => {
                                    fx_assets.set_allow_asset_overrides(
                                        request.prepare_options.allow_asset_overrides,
                                    );
                                    let key = FxCatalogKey {
                                        game: request.game.clone(),
                                        allow_asset_overrides: request.prepare_options.allow_asset_overrides,
                                        pbr_materials: request.prepare_options.pbr_materials,
                                        index: fx_assets.index_token(),
                                    };
                                    let cached = fx_catalog_cache
                                        .as_ref()
                                        .filter(|(cached, _)| *cached == key)
                                        .map(|(_, catalog)| Arc::clone(catalog));
                                    if let Some(catalog) = cached {
                                        jobs.mark_done(crate::thread_activity::Task::MapFxLightProfiles);
                                        println!(
                                            "{name}: FX light promotion reused cached catalog ({} EFX / {} material(s))",
                                            catalog.scanned_effects(),
                                            catalog.analyzed_materials(),
                                        );
                                        catalog
                                    } else {
                                        let effect_names = if request.analyze_fx_light_profiles {
                                            crate::fx::light_promotion::effect_asset_names(&fx_assets)
                                        } else {
                                            // Profile only this scene's runners on the map
                                            // worker; reuse the regular classifier and avoid
                                            // scanning all game FX for the menu.
                                            crate::fx::light_promotion::runner_effect_asset_names(&map.fx_runners)
                                        };
                                        let total = effect_names.len().max(1).min(u32::MAX as usize) as u32;
                                        let shaders = Arc::clone(&map.shader_library);
                                        match jobs.submit_progress(
                                            crate::thread_activity::Task::MapFxLightProfiles,
                                            total,
                                            move |progress| {
                                                crate::fx::light_promotion::build_catalog(
                                                    &mut fx_assets,
                                                    shaders.as_ref(),
                                                    &effect_names,
                                                    |completed, _total| {
                                                        progress.set_completed(completed);
                                                    },
                                                )
                                            },
                                        ) {
                                            Ok(job) => match job.join() {
                                                Ok(catalog) => {
                                                    println!(
                                                        "{name}: FX light promotion profiled {} EFX / {} semantic candidates / {} promotable effects / {} material(s) in {:.1} ms",
                                                        catalog.scanned_effects(),
                                                        catalog.semantic_candidates(),
                                                        catalog.promotable_effects(),
                                                        catalog.analyzed_materials(),
                                                        profile_started.elapsed().as_secs_f64() * 1000.0,
                                                    );
                                                    let catalog = Arc::new(catalog);
                                                    if request.analyze_fx_light_profiles {
                                                        fx_catalog_cache = Some((key, Arc::clone(&catalog)));
                                                    }
                                                    catalog
                                                }
                                                Err(error) => {
                                                    eprintln!(
                                                        "{name}: FX light promotion worker failed: {error}"
                                                    );
                                                    Arc::new(
                                                        crate::fx::light_promotion::FxLightPromotionCatalog::default(),
                                                    )
                                                }
                                            },
                                            Err(error) => {
                                                eprintln!(
                                                    "{name}: could not queue FX light promotion worker: {error}"
                                                );
                                                jobs.mark_done(
                                                    crate::thread_activity::Task::MapFxLightProfiles,
                                                );
                                                Arc::new(
                                                    crate::fx::light_promotion::FxLightPromotionCatalog::default(),
                                                )
                                            }
                                        }
                                    }
                                }
                                Err(error) => {
                                    eprintln!(
                                        "{name}: FX light promotion asset path unavailable: {error}"
                                    );
                                    jobs.mark_done(
                                        crate::thread_activity::Task::MapFxLightProfiles,
                                    );
                                    Arc::new(
                                        crate::fx::light_promotion::FxLightPromotionCatalog::default(),
                                    )
                                }
                            }
                        } else {
                            Arc::new(
                                crate::fx::light_promotion::FxLightPromotionCatalog::default(),
                            )
                        };

                        map.static_bake_lights.extend(fx_light_promotion.static_runner_lights(&map.fx_runners));

                        // A cache hit is already attached to `map`. A cache miss
                        // becomes a detached map-worker job only after the prepared
                        // world has been delivered, so first-time offline baking can
                        // never hold up joining a live server.
                        let bake_request = map.steam_audio_bake_request.take();
                        if jobs.is_superseded() {
                            continue;
                        }
                        if proxy
                            .send_event(UserEvent::MapPrepared {
                                request_id: request.request_id,
                                started: request.started,
                                name: name.clone(),
                                map: Box::new(map),
                                fx_light_promotion,
                            })
                            .is_err()
                        {
                            return;
                        }

                        if let Some(bake_request) = bake_request {
                            let request_id = request.request_id;
                            let map_name = name.clone();
                            let progress_proxy = proxy.clone();
                            let finish_proxy = proxy.clone();
                            let started = Instant::now();
                            let submit_result = jobs.submit(
                                crate::thread_activity::Task::MapAudioBake,
                                move || {
                                    let _ = progress_proxy.send_event(
                                        UserEvent::SteamAudioBakeProgress {
                                            request_id,
                                            map_name: map_name.clone(),
                                            progress: 0.0,
                                        },
                                    );
                                    let callback_proxy = progress_proxy.clone();
                                    let callback_map_name = map_name.clone();
                                    let progress: crate::steam_audio::SteamAudioBakeProgress =
                                        Arc::new(move |fraction| {
                                            let _ = callback_proxy.send_event(
                                                UserEvent::SteamAudioBakeProgress {
                                                    request_id,
                                                    map_name: callback_map_name.clone(),
                                                    progress: fraction,
                                                },
                                            );
                                        });
                                    let result = std::panic::catch_unwind(
                                        std::panic::AssertUnwindSafe(|| {
                                            crate::steam_audio::load_or_bake(
                                                bake_request.mesh.as_ref(),
                                                &bake_request.cache,
                                                bake_request.num_threads,
                                                Some(progress),
                                            )
                                        }),
                                    )
                                    .map_err(|_| "Steam Audio bake worker panicked".to_string())
                                    .and_then(|result| result);
                                    let _ = finish_proxy.send_event(
                                        UserEvent::SteamAudioBakeFinished {
                                            request_id,
                                            map_name,
                                            result,
                                            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                                        },
                                    );
                                },
                            );
                            if let Err(error) = submit_result {
                                let _ = proxy.send_event(UserEvent::SteamAudioBakeFinished {
                                    request_id,
                                    map_name: name,
                                    result: Err(format!(
                                        "could not queue Steam Audio background bake: {error}"
                                    )),
                                    elapsed_ms: 0.0,
                                });
                            }
                        }
                    }
                    Err(error) => {
                        if proxy
                            .send_event(UserEvent::MapFailed {
                                request_id: request.request_id,
                                started: request.started,
                                name,
                                error,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        })
        .map_err(|e| format!("Could not start map loader: {e}"))?;
    Ok(tx)
}
