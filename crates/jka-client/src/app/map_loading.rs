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
    pub(in crate::app) prepare_options: scene::MapPrepareOptions,
    /// The already-prepared copy of this same map (the restart cache), lent so
    /// the loader can copy unchanged stages instead of recomputing them.
    pub(in crate::app) seed: Option<Arc<scene::PreparedMap>>,
}

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
}

pub(in crate::app) struct MapLoadingState {
    pub(in crate::app) request_id: u64,
    pub(in crate::app) name: String,
    pub(in crate::app) active_game_dir: Option<String>,
    pub(in crate::app) preparation_finished: bool,
    pub(in crate::app) bars: [MapLoadingBarState; 11],
}

impl MapLoadingState {
    pub(in crate::app) fn new(request_id: u64, name: String, active_game: Option<&Path>) -> Self {
        use crate::thread_activity::Task;
        Self {
            request_id,
            name,
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
            picmip: self.video.picmip,
            grass: self.video.grass,
            voxel_probe_gi: self.video.voxel_probe_gi,
            ocean: self.video.ocean,
            client_physics: self.video.client_physics,
            gen_normal_maps: self.video.gen_normal_maps,
            float_lightmap: self.video.float_lightmap && self.video.hdr,
            planar_reflections: self.video.reflection_quality.planar_slot_budget() > 0,
            planar_environment: self.video.reflection_quality.promotes_environment_planars(),
            omit_environment_stages: self.video.reflection_quality.omits_environment_stages(),
            source_spatial_batches: self.video.gpu_driven,
            pbr_materials: self.video.pbr,
            allow_asset_overrides: self.video.allow_asset_overrides,
            steam_audio: self.audio.steam_audio,
        }
    }

    pub(in crate::app) fn request_map(&mut self, source: scene::MapSource) {
        self.request_map_internal(source, MapLoadPurpose::Gameplay);
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
                seed: None,
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
                editor.poll_external_change(now)
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
        self.request_map_internal(
            scene::MapSource::Bsp(FRONTEND_BACKGROUND_MAP.to_owned()),
            MapLoadPurpose::FrontendBackground,
        );
    }

    pub(in crate::app) fn frontend_background_prepare_options(&self) -> scene::MapPrepareOptions {
        let mut options = self.current_map_prepare_options();
        // The menu scene is visual-only. Do not build gameplay-only collision or
        // kick an offline Steam Audio bake simply because duel3 is on screen.
        options.client_physics = false;
        options.steam_audio = false;
        options
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
        self.map_edit_preview_request_id = None;
        self.map_edit_preview_pending_collision = None;
        self.map_edit_preview_dirty = false;
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
            .map(|cache| Arc::clone(&cache.map));
        if self
            .loader_tx
            .send(MapLoadRequest {
                request_id,
                started: Instant::now(),
                game: self.game.clone(),
                source,
                prepare_options,
                seed,
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
            (cache.label == label && prepared == prepare_options)
                .then(|| Box::new(cache.map.as_ref().clone()))
        }) else {
            return false;
        };
        let clone_ms = clone_started.elapsed().as_secs_f64() * 1000.0;

        // These timings describe CPU preparation from the original load. A cached
        // renderer restart did none of that work, so keep the restart load report
        // honest and show only its fresh GPU upload/first-frame cost.
        map.load_timings = scene::MapLoadTimings::default();

        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        let request_id = self.latest_request_id;
        let started = Instant::now();
        self.static_ao_progress = None;
        self.preserve_game_state_on_next_map_upload = false;
        let mut loading = MapLoadingState::new(request_id, label.clone(), self.game.as_deref());
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
        });
        self.publish_snapshot();
        true
    }
}

pub(in crate::app) fn spawn_map_loader(
    base: PathBuf,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<Sender<MapLoadRequest>, String> {
    let (tx, rx) = mpsc::channel::<MapLoadRequest>();
    thread::Builder::new()
        .name("jka-map-loader".into())
        .spawn(move || {
            let progress_proxy = proxy.clone();
            let jobs = match crate::map_jobs::MapJobPool::new_with_progress(move |progress| {
                let _ = progress_proxy.send_event(UserEvent::MapLoadProgress {
                    request_id: progress.request_id,
                    task: progress.task,
                    completed: progress.completed,
                    total: progress.total,
                });
            }) {
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
            while let Ok(request) = rx.recv() {
                jobs.begin_request(request.request_id);
                let name = request.source.label();
                let _activity = crate::thread_activity::activity(
                    crate::thread_activity::ThreadSlot::MapLoader,
                    crate::thread_activity::Task::MapPrepare,
                );
                match scene::prepare_source_with_jobs(
                    &base,
                    request.game.as_deref(),
                    &request.source,
                    &jobs,
                    request.prepare_options,
                    request.seed.as_ref(),
                ) {
                    Ok(mut map) => {
                        // A cache hit is already attached to `map`. A cache miss
                        // becomes a detached map-worker job only after the prepared
                        // world has been delivered, so first-time offline baking can
                        // never hold up joining a live server.
                        let bake_request = map.steam_audio_bake_request.take();
                        if proxy
                            .send_event(UserEvent::MapPrepared {
                                request_id: request.request_id,
                                started: request.started,
                                name: name.clone(),
                                map: Box::new(map),
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
