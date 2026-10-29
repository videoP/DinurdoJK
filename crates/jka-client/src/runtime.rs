use crate::{
    scene::{BspMapStats, MapLoadTimings, PreparedMap},
    thread_activity::{Task, ThreadActivitySample},
    ui::VsyncMode,
};
use std::{path::PathBuf, sync::Arc, time::Instant};

#[derive(Clone, Debug, Default)]
pub struct LoadProgressState {
    pub label: String,
    pub completed: u64,
    pub total: u64,
}
#[derive(Default)]
pub struct LoadProgress(std::sync::Mutex<LoadProgressState>);
impl LoadProgress {
    pub fn set(&self, label: impl Into<String>, completed: u64, total: u64) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = LoadProgressState { label: label.into(), completed, total };
    }
    pub fn snapshot(&self) -> LoadProgressState { self.0.lock().unwrap_or_else(|e| e.into_inner()).clone() }
}

#[derive(Debug, Clone)]
pub struct RenderStats {
    pub fps: f64,
    pub frame_ms: f64,
    pub cpu_prepare_ms: f64,
    pub cpu_acquire_ms: f64,
    pub cpu_encode_ms: f64,
    pub cpu_submit_ms: f64,
    pub cpu_present_ms: f64,
    pub dynamic_model_prepare_ms: f64,
    pub dynamic_model_surfaces: u32,
    pub dynamic_model_vertices: u64,
    pub dynamic_model_indices: u64,
    pub gpu_ms: Option<f64>,
    pub gpu_depth_ms: Option<f64>,
    pub gpu_hiz_ms: Option<f64>,
    pub gpu_cull_ms: Option<f64>,
    pub gpu_cluster_ms: Option<f64>,
    pub gpu_world_ms: Option<f64>,
    pub gpu_fx_sprites_ms: Option<f64>,
    pub gpu_post_ms: Option<f64>,
    pub gpu_ui_ms: Option<f64>,
    pub cull_visible: u32,
    pub cull_frustum_rejected: u32,
    pub cull_hiz_rejected: u32,
    pub cull_pvs_rejected: u32,
    pub cull_area_rejected: u32,
    /// Main world pass program binds last frame: specialized, general.
    pub world_program_binds: (u32, u32),
    pub world_shader_programs: WorldShaderPrograms,
    pub input_event_to_sim_ms: Option<f64>,
    pub input_sim_to_render_ms: Option<f64>,
    pub input_event_to_latch_ms: Option<f64>,
    pub input_latch_to_submit_ms: Option<f64>,
    pub input_latch_to_present_call_ms: Option<f64>,
    pub input_event_to_present_call_ms: Option<f64>,
    pub input_event_to_present_call_max_ms: Option<f64>,
    pub input_latency_samples: u32,
    pub msaa_samples: u32,
    pub vsync: VsyncMode,
    pub threads: Arc<[ThreadActivitySample]>,
}

/// Program binds are actual main-pass state changes, not estimated sort savings.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorldShaderPrograms {
    pub requested: u8,
    pub active: u8,
    /// General, specialized, mode-2 minimal, shared, generated.
    pub binds: [u32; 5],
    /// A requested world variant is still compiling in the background.
    pub pending: bool,
}

#[derive(Debug, Clone)]
pub struct SurfaceInspectorSection {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SurfaceInspectorInfo {
    /// Human-facing category shown in the trace panel (WORLD SURFACE, ENTITY, ...).
    pub kind: String,
    /// Primary identity: material name, entity/model label, etc.
    pub title: String,
    /// Small set of high-value fields rendered prominently as label/value rows.
    pub summary: Vec<(String, String)>,
    /// Human-oriented grouped details. The raw diagnostic dump stays in `lines`.
    pub sections: Vec<SurfaceInspectorSection>,
    /// Complete diagnostic dump exported by Ctrl+C from the Trace popup.
    pub lines: Vec<String>,
    /// Protocol entity hit by the trace, when applicable. App-side enrichment uses
    /// this to add classname/spawn vars/client state without moving game state to
    /// the render thread.
    pub hit_entity_num: Option<u16>,
    /// Inline BSP model number (`*N`) for brush-model hits.
    pub hit_inline_model: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct WorldUploadTimings {
    /// A Render Studio world swap reused resident geometry and textures.
    pub studio_world_reused: bool,
    /// A remap requested new geometry and required the compatibility full-world path.
    pub remap_full_fallback: bool,
    pub vertex_buffer_ms: f64,
    /// Indexed-geometry CPU preparation and patch LOD generation.
    pub vertex_cpu_ms: f64,
    /// WGPU buffer creation and queue submission on the calling thread.
    pub vertex_gpu_calls_ms: f64,
    /// Material-usage classification before world upload (small CPU cost).
    pub texture_classify_ms: f64,
    /// Skybox image CPU averaging, before texture WGPU calls.
    pub skybox_average_ms: f64,
    /// Number of primary skybox face slots sampled (0 or 6, never PVS-dependent).
    pub skybox_face_references: usize,
    /// Number of distinct skybox texture indices averaged/cached.
    pub skybox_unique_images: usize,
    /// Worker compute time; can overlap the vertex stage, not additive to load time.
    pub texture_encode_cpu_ms: f64,
    /// Actual render-thread time awaiting the colour/companion BC worker.
    pub texture_encode_wait_ms: f64,
    /// WGPU image creation/queue API time; NOT measured GPU execution.
    pub texture_gpu_calls_ms: f64,
    pub lightmap_encode_cpu_ms: f64,
    pub lightmap_encode_wait_ms: f64,
    pub lightmap_gpu_calls_ms: f64,
    pub texture_upload_ms: f64,
    pub lightmap_upload_ms: f64,
    pub reflection_probe_upload_ms: f64,
    pub batch_bind_groups_ms: f64,
    pub cull_resources_ms: f64,
    pub lighting_resources_ms: f64,
    pub pipeline_create_ms: f64,
    pub visibility_tables_ms: f64,
    pub grass_upload_ms: f64,
    pub finalize_ms: f64,
    /// `load_map` work before `build_world` (fog install, detail textures).
    pub pre_build_ms: f64,
    /// Snow-shell mesh build at the top of `build_world`.
    pub snow_shell_ms: f64,
    /// CPU-side copies for the inspector and static AO, before the vertex upload.
    pub prelude_ms: f64,
    /// Cull and froxel bind groups created after `build_world` returns.
    pub post_bind_groups_ms: f64,
    /// Videos, light buffer, static AO request, weather and fog sync.
    pub post_misc_ms: f64,
    /// Planar reflection resources and `rebuild_frame_plan`.
    pub frame_plan_ms: f64,
    /// `activate_world_pipeline_variant`.
    pub variant_ms: f64,
    /// FFT-ocean geometry report. The promoted water surface only shows wave
    /// displacement if it carries the dense replacement index range, so these
    /// distinguish "wrong spectrum" from "never tessellated".
    pub ocean_water_batches: u32,
    pub promoted_oceans: Vec<crate::ocean::appearance::PromotedOcean>,
    pub ocean_clipmap_vertices: u64,
    pub ocean_clipmap_tris: u64,
    pub ocean_clipmap_spacing: f32,
    pub ocean_coarsest_spacing: f32,
}

#[derive(Debug)]
pub enum ScreenshotOutput {
    Saved(PathBuf),
    FrontendSceneSaved(PathBuf),
    Clipboard,
    VideoFrame {
        frame_index: u64,
        width: u32,
        height: u32,
        bgra: Vec<u8>,
        depth_f32le: Option<Vec<u8>>,
        timing: VideoFrameTiming,
    },
}

/// Recycled byte buffers for video-frame readbacks (Render Studio export).
/// The FFmpeg writer hands each frame back after its pipe write, so the next
/// readback copies into resident pages instead of faulting in a fresh one.
static VIDEO_FRAME_POOL: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());

/// An empty buffer with at least `len` bytes of capacity, recycled if possible.
pub fn take_video_frame_buffer(len: usize) -> Vec<u8> {
    let mut pool = VIDEO_FRAME_POOL.lock().unwrap_or_else(|error| error.into_inner());
    match pool.iter().position(|buffer| buffer.capacity() >= len) {
        Some(index) => {
            let mut buffer = pool.swap_remove(index);
            buffer.clear();
            buffer
        }
        None => Vec::with_capacity(len),
    }
}

pub fn recycle_video_frame_buffer(buffer: Vec<u8>) {
    let mut pool = VIDEO_FRAME_POOL.lock().unwrap_or_else(|error| error.into_inner());
    if pool.len() < 8 { pool.push(buffer); }
}

/// Release pooled frame memory once an export ends.
pub fn clear_video_frame_pool() {
    VIDEO_FRAME_POOL.lock().unwrap_or_else(|error| error.into_inner()).clear();
}

/// Renderer-side milestones of one video-frame capture, for export profiling.
/// Consecutive instants; `gpu_wait_ms`/`readback_copy_ms` split presented..queued.
#[derive(Clone, Copy, Debug)]
pub struct VideoFrameTiming {
    /// CaptureVideoFrame was handled by the render thread.
    pub received: Instant,
    /// The capturing render frame prepared its readback (after surface acquire).
    pub frame_started: Instant,
    /// That frame was recorded, submitted and presented; readback mapping begins.
    pub presented: Instant,
    pub gpu_wait_ms: f64,
    pub readback_copy_ms: f64,
    /// Mapped pixels were handed to the encode worker.
    pub queued: Instant,
    pub worker_started: Instant,
    pub worker_done: Instant,
}

pub enum UserEvent {
    Update(crate::updater::Event),
    BlueWaterThunder(f32),
    /// Credential Manager enumeration completes on a separate thread; the main
    /// window must not wait for it to show its first interactive frame.
    SavedAccountsReady(Result<Vec<crate::credential_store::SavedAccount>, String>),
    ConsoleCommand(String),
    GammaPrepared(u64),
    HardwareGamma(crate::display_gamma::Status),
    ScreenshotFinished(Result<ScreenshotOutput, String>),
    /// `vram` is process dedicated-VRAM (usage, OS budget) after the scene
    /// settled; `scenes` counts the retained Studio scenes at that point.
    StudioSourceReady { id: u64, error: Option<String>, vram: Option<(u64, u64)>, scenes: usize },
    #[cfg(test)]
    StudioTestScenePresented { overlap: bool },
    HdrOutputChanged { status: String, active: bool, available_modes: u8 },
    RendererReady {
        supported_msaa: Vec<u32>,
        wireframe_supported: bool,
        hdr_output_status: String,
        hdr_output_active: bool,
        hdr_available_modes: u8,
        adapter_name: String,
        adapter_type: String,
    },
    CompanionReady { id: u32 },
    CompanionError { id: u32, error: String },
    RendererFirstFrame,
    /// Emitted only after a frame with egui has successfully presented.
    RendererFirstMenuFrame,
    /// The renderer presented a frame whose egui paint data carried this token.
    /// Sent once per distinct token; drives the menu stage fade clock.
    FrontendStagePresented(u64),
    /// WGPU main-menu effects are compositing (egui can stop painting its fallback still).
    NeonMenuCompositorReady,
    /// An asynchronously decoded placeholder (not the live 3D scene).
    FrontendSceneCacheReady { path: PathBuf, generation: u64, image: Option<egui::ColorImage> },
    /// Footsteps and landings the renderer found in standing water; the game
    /// thread plays the engine's own splash effects for them.
    WaterSplashes(Vec<crate::weather::wake::WaterSplash>),
    RenderStats(RenderStats),
    RendererError(String),
    SurfaceInspected(Option<SurfaceInspectorInfo>),
    MapLoadProgress {
        request_id: u64,
        task: Task,
        completed: u32,
        total: u32,
    },
    StaticAoProgress {
        completed: u32,
        total: u32,
    },
    /// Targeted current-BSP acoustic preparation requested by `snd_restart`.
    /// This does not rebuild or upload the rendered world.
    SteamAudioScenePrepared {
        request_id: u64,
        map_name: String,
        result: Result<crate::scene::PreparedSteamAudioScene, String>,
    },
    SteamAudioBakeProgress {
        request_id: u64,
        map_name: String,
        progress: f32,
    },
    SteamAudioBakeFinished {
        request_id: u64,
        map_name: String,
        result: Result<crate::steam_audio::SteamAudioBakeData, String>,
        elapsed_ms: f64,
    },
    /// A worker-resolved remap stage plan. Not a map load: no BSP, collision,
    /// or texture source allocations were cloned while preparing this message.
    ShaderRemapPrepared {
        request_id: u64,
        started: Instant,
        name: String,
        base_texture_count: usize,
        result: Result<Box<crate::scene::PreparedRemapPlan>, String>,
    },
    MapPrepared {
        request_id: u64,
        started: Instant,
        name: String,
        map: Box<PreparedMap>,
        fx_light_promotion: Arc<crate::fx::light_promotion::FxLightPromotionCatalog>,
    },
    /// Rapier static-world mesh built on a worker after client physics was
    /// enabled on an already-loaded map.
    PhysicsMeshReady {
        label: String,
        result: Result<crate::cgame::ragdoll::PhysicsMapMesh, String>,
    },
    MapFailed {
        request_id: u64,
        started: Instant,
        name: String,
        error: String,
    },
    WorldUploaded {
        request_id: u64,
        name: String,
        triangles: usize,
        batches: usize,
        upload_ms: f64,
        upload_timings: WorldUploadTimings,
        first_frame_ms: f64,
        timings: MapLoadTimings,
        bsp_stats: Option<BspMapStats>,
    },
}
