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
pub struct SurfaceInspectorInfo {
    pub title: String,
    pub lines: Vec<String>,
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
    /// FFT-ocean geometry report. The promoted water surface only shows wave
    /// displacement if it carries the dense replacement index range, so these
    /// distinguish "wrong spectrum" from "never tessellated".
    pub ocean_water_batches: u32,
    pub ocean_clipmap_vertices: u64,
    pub ocean_clipmap_tris: u64,
    pub ocean_clipmap_spacing: f32,
    pub ocean_coarsest_spacing: f32,
}

pub enum UserEvent {
    ConsoleCommand(String),
    ScreenshotFinished(Result<(PathBuf, Option<String>), String>),
    RendererReady {
        supported_msaa: Vec<u32>,
        wireframe_supported: bool,
    },
    RendererFirstFrame,
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
