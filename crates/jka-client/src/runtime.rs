use crate::{
    scene::{BspMapStats, MapLoadTimings, PreparedMap},
    thread_activity::{Task, ThreadActivitySample, SLOT_COUNT},
    ui::VsyncMode,
};
use std::{path::PathBuf, time::Instant};

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
    pub threads: [ThreadActivitySample; SLOT_COUNT],
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

#[derive(Debug, Clone, Copy, Default)]
pub struct WorldUploadTimings {
    pub vertex_buffer_ms: f64,
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
    pub ocean_clipmap_vertices: u64,
    pub ocean_clipmap_tris: u64,
    pub ocean_clipmap_spacing: f32,
    pub ocean_coarsest_spacing: f32,
}

#[derive(Debug)]
pub enum ScreenshotOutput {
    Saved(PathBuf),
    Clipboard,
}

pub enum UserEvent {
    ConsoleCommand(String),
    GammaPrepared(u64),
    HardwareGamma(crate::display_gamma::Status),
    ScreenshotFinished(Result<ScreenshotOutput, String>),
    RendererReady {
        supported_msaa: Vec<u32>,
        wireframe_supported: bool,
    },
    CompanionReady { id: u32 },
    CompanionError { id: u32, error: String },
    RendererFirstFrame,
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
    MapPrepared {
        request_id: u64,
        started: Instant,
        name: String,
        map: Box<PreparedMap>,
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
