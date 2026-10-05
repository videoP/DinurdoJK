//! renderer module facade. Implementations are grouped by responsibility.
use api::{FrameInfo, RenderError};
use frame::plan::FramePlan;
use lighting::legacy_dlights::legacy_dlight_runs_for_batch;
use overlay::ScreenFxBatch;
use world::videos::{WorldVideoPlayer, WORLD_VIDEO_MAX_CATCH_UP};
mod api;
mod bindings;
mod capture;
mod companion;
mod context;
mod diagnostics;
mod environment;
mod frame;
mod initialize;
mod lighting;
mod models;
mod overlay;
mod post;
mod reflections;
mod render_thread;
mod static_ao;
mod targets;
mod textures;
mod brightness;
mod brightness_settings;
mod uniforms;
mod visibility;
mod world;
use api::{
    dynamic_model_writes_depth, Ghoul2RtInputVertex, LegacyDlightPerfStats, RenderBatchTiming,
    DYNAMIC_MODEL_ALPHA_ORDER, MAX_CLOUD_FOREGROUND_BLADES,
};
pub use api::{
    ClientFramePerf, CloudForegroundBlade, CompanionId, CompanionSceneView, DynamicModelAlphaMode,
    DynamicModelSurface, DynamicModelVertex, DynamicWireframeClass, EguiRenderData,
    FxGpuSpriteInstance, FxGpuSprites, Ghoul2GpuBone, Ghoul2GpuSkinning, Ghoul2GpuVertex,
    InlineModelInstance, InspectorEntityHint, PostEffects, RayTracedRigidInstance, RenderCommand,
    RenderSnapshot, TransientLight, PBR_PROFILE_COMPANION_COMPRESSION,
    PBR_PROFILE_COMPANION_SAMPLER_TRILINEAR, PBR_PROFILE_MATERIALS, PBR_PROFILE_PARALLAX_OCCLUSION,
    PBR_PROFILE_POM_ADAPTIVE_STEPS, PBR_PROFILE_POM_MIP_AWARE, PBR_PROFILE_SHARED_MATERIAL_EVAL,
    PBR_PROFILE_SHARED_TANGENT_FRAME, PBR_PROFILE_VERTEX_LIGHTGRID,
};
use bindings::{
    comparison_sampler_entry, cube_texture_entry, depth_array_texture_entry,
    depth_cube_array_texture_entry, depth_texture_entry, nonfiltering_sampler_entry, sampler_entry,
    sampler_entry_stages, storage_buffer_entry, texture_2d_array_entry, texture_3d_entry,
    texture_3d_entry_stages, texture_entry, texture_entry_stages, unfilterable_texture_entry,
    unfilterable_texture_entry_stages, uniform_entry,
};
use capture::{
    encode_screenshot_job, ScreenshotDestination, ScreenshotEncodeJob, ScreenshotReadbackBuffer,
};
use companion::{CompanionRenderThread, CompanionSceneFrame};
use context::{no_vsync_mode, supported_msaa, temporal_jitter};
use diagnostics::debug_geometry::{
    create_screen_fx_pipeline, create_surface_inspector_pipeline, create_ui_pipeline,
    DebugVolumeRenderer,
};
#[cfg(test)]
use diagnostics::debug_geometry::{sun_ray_vertices, SUN_RAY_LENGTH};
use diagnostics::inspector::{inspector_texture_meta, InspectorTextureMeta, InspectorVertex};
use diagnostics::profiling::{
    GpuPass, GpuProfiler, CULL_DIAGNOSTIC_COUNTER_BYTES, CULL_DIAGNOSTIC_READBACK_SLOTS,
};
use environment::clouds::{create_cloud_fallback_resources, CloudNoiseResources};
use environment::fog::create_world_froxel_bind_group;
use environment::snow::{
    create_snow_shell_gpu, draw_snow_shell, draw_snow_shell_fast, gpu_vertex_key,
    snow_shell_draw_near_center, SnowShellGpu,
};
use environment::surface_sprites::{
    ghoul2_specular_light, ghoul2_specular_viewer, PreparedSurfaceSpriteEffects,
    SurfaceSpriteEffectGpu, SurfaceSpriteEffectRenderer, DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
    FX_GPU_SPRITE_INSTANCE_ATTRIBUTES, GHOUL2_GPU_VERTEX_ATTRIBUTES,
};
use environment::water::{
    is_upward_water_face, ocean_surface_key, water_surface_extent, MAX_OCEAN_SURFACES,
};
use frame::late_latch::{sample_view_rotation, LatestViewState, ViewSampleMeasurement};
pub use frame::late_latch::{InputLatencySample, ViewLatchMode};
use lighting::cascades::{bevy_cascade_bounds, bevy_cascade_shadow_matrices};
#[cfg(test)]
use lighting::entity_shadows::best_authored_light_direction;
use lighting::entity_shadows::EntityShadowState;
use lighting::legacy_dlights::{
    barycentric_coordinates, legacy_dlight_diagnostic, legacy_dlight_surface_mask, ray_hits_aabb,
    vertex_dlight_cpu,
};
use lighting::local_shadows::{
    aabb_behind_plane, aabb_intersects_clip_frustum, aabb_intersects_shadow_frustum,
    create_local_shadow_resources, gpu_point_lights,
};
use lighting::ray_tracing::{
    compose_world_shader, create_rt_receiver_bind_group, create_shadow_receiver_bind_group,
    ray_traced_world_shader_source, rt_alpha_mask_source_from_gpu_vertices,
    rt_alpha_storage_entries, rt_sampled_light_loops, RayTracedRigidMeshKey,
    RayTracedShadowResources, RtAlphaMaskSource,
};
#[cfg(test)]
use lighting::ray_tracing::{
    create_ray_traced_shadow_receiver_layout, create_rt_alpha_buffer, create_rt_triangle_blas,
    rt_alpha_dynamic_material, rt_alpha_empty_buffer_descriptor, rt_alpha_ensure_dynamic_texture,
    rt_alpha_texture_key, rt_triangle_blas_build_entry, RtAlphaGeometryGpu, RtAlphaMaterialGpu,
    RtAlphaTextureGpu, RtAlphaVertexGpu,
};
use lighting::shadow_resources::{
    create_shadow_mask_pipeline, create_shadow_pipeline, create_shadow_resources,
    create_shadow_translucent_pipeline, create_sky_admission_pipeline, LocalShadowCacheEntry,
    LocalShadowResources, ShadowResources,
};
use lighting::upload::{upload_reflection_probes, upload_static_light_grid, upload_voxel_probe_gi};
use models::pipelines::{
    create_dynamic_model_pipeline_inner, create_fast_world_wireframe_pipeline,
    create_world_pipeline, create_world_wireframe_pipeline, project_blob_shadow_mark,
};
pub(crate) use models::types::EntityLegacyFog;
use models::types::{
    classic_vertex_illumination, sample_entity_classic_light, DynamicEntityLightingUniform,
    DynamicGpuTexture, DynamicModelRenderer, DynamicPreparedDraw, EntitySunRelight,
    FxGpuSpritePreparedDraw, Ghoul2GpuDrawUniform, Ghoul2GpuMesh, Ghoul2PendingDraw,
    Ghoul2PreparedDraw, Ghoul2RtComputeBatch, Ghoul2RtComputeMesh, Ghoul2RtSkinningResources,
    RayTracedSkinnedMeshKey, RayTracedSkinnedPrepared, BLOB_MARK_PROJECTION,
    BLOB_MARK_SURFACE_LIFT,
};
use overlay::{
    empty_ui_font_texture, load_ui_font_texture, load_ui_loading_texture,
    load_ui_proportional_font, load_ui_unknown_map_texture, PendingUiLevelshot, ScreenFxTexture,
};
use post::bindings::{
    create_auto_exposure_bind_group, create_bloom_bind_groups, create_dof_bind_group,
    create_fast_post_bind_group, create_gamma_post_bind_group, create_post_bind_group,
    create_ssao_temporal_bind_groups, create_ssr_temporal_bind_groups,
};
use post::color_lut::create_color_lut_texture;
use post::pipelines::{
    create_bloom_pipeline, create_cloud_pass_pipeline, create_cloud_resolve_bind_group,
    create_cull_debug_pipeline, create_dof_pipeline, create_gamma_post_pipeline,
    create_planar_debug_pipeline, create_post_pipeline, create_ssao_temporal_pipeline,
    create_ssr_temporal_pipeline, create_ssr_visibility_resources, create_taa_post_pipeline,
    create_wireframe_pipeline,
};
use post::targets::{
    create_bloom_pyramid, create_dof_target, create_temporal_ssao_history,
    create_temporal_ssr_history,
};
#[cfg(test)]
use reflections::planar::projected_planar_importance;
#[cfg(test)]
use reflections::planar::reflection_matrix;
use reflections::planar::{
    collect_planar_reflectors, create_planar_reflection_resources, select_planar_reflection_views,
};
pub use render_thread::RenderThread;
use static_ao::bake::{
    collect_static_ao_lightmap_triangles, collect_static_ao_render_triangles,
    collect_static_ao_vertex_receivers, run_static_ao_worker,
};
use static_ao::bvh::{
    bake_static_structural_vertex_ao, dilate_static_ao_mask, dilate_static_ao_owners,
    rasterize_static_ao_owners, rasterize_static_ao_texels, spread_static_ao_mask,
    static_ao_cache_path, static_ao_hq_visibility, static_ao_sample_pattern,
    static_ao_worker_count, StaticAoBvh,
};
use static_ao::types::{
    StaticAoBakeData, StaticAoBakeMode, StaticAoBakedLightmap, StaticAoBaseLightmap,
    StaticAoJobKey, StaticAoLightmapTriangle, StaticAoPendingJob, StaticAoRenderTriangle,
    StaticAoTexelSample, StaticAoTraceTriangle, StaticAoWorkerResult, StaticAoWorkerUpdate,
    StaticAoWorldSource,
};
use targets::{
    create_menu_backdrop_resources, create_targets, BloomPyramid, CompanionSceneSlot,
    CompanionSceneTarget, DofTarget, MenuBackdropResources, RenderTargets, TemporalSsaoHistory,
    TemporalSsrHistory, COMPANION_SCENE_FRAME_INTERVAL, COMPANION_SCENE_RING_SIZE,
};
use textures::{
    create_shader_module_timed, create_texture_sampler, gpu_blend_factor,
    load_texture_asset_or_missing, load_ui_key_atlas, timed_init_step, try_load_texture_asset,
    try_load_texture_asset_from_search_path, try_load_ui_font_asset, upload_texture,
    upload_texture_bc3_picmip, upload_texture_picmip, GpuImage,
};
use uniforms::{
    transient_light_gpu, transient_light_samples, AutoExposureState, AutoExposureUniform,
    CameraUniform, CloudHistoryKey, CloudLayerSettings, CloudRenderSettings, CloudSunSettings,
    DofUniform, DrawIndexedIndirectArgs, GammaPostUniform, GpuCullRecord, GpuCullSettings,
    GpuPointLight, IrradianceVolumeUniform, LightingSettings, MaterialUniform, PbrSettings,
    PlanarReflectionUniform, PostAaSettings, PostCameraFxSettings, PostCloudShadowSettings,
    PostCloudShapingSettings, PostCloudSkyAmbientSettings, PostCloudTemporalSettings,
    PostCloudTemporalTuningSettings, PostCloudVariationSettings, PostCloudWindSettings,
    PostColorSettings, PostFilmSettings, PostGrainSettings, PostSceneSettings, PostUniform,
    ShadowCasterUniform, ShadowReceiverUniform, SsaoTemporalUniform, SsrTemporalUniform,
    StaticLightGridUniform, VoxelProbeGiUniform, ENTITY_SHADOW_CAMERAS, ENTITY_SHADOW_LIGHT_RANGE,
    ENTITY_SHADOW_MAX_STRENGTH, ENTITY_SHADOW_MIN_ELEVATION, ENTITY_SHADOW_RADIUS,
    ENTITY_SHADOW_SMOOTHING_SECONDS,
};
use visibility::bindings::{
    create_cull_debug_bind_group, create_gpu_cull_bind_group, create_hiz_build_bind_group,
    create_hiz_reduce_bind_groups,
};
use visibility::compaction::{
    auto4_accept_collapse, instantiate_portal_draw_plans, Auto4CollapseCache, Auto4CollapseRequest,
    Auto4CollapseWorker, Auto4GeometryState, Auto4Instance, Auto4PlanStats, IndexedGeometryBuilder,
};
use visibility::pvs::{
    active_batch_selection, batch_area_visible, effective_area_mask, legacy_dlight_batch_selection,
    refresh_auto4_lazy_collapse, update_active_cull_indices, update_visibility,
    visible_batches_by_cluster,
};
use visibility::readback::CullDiagnosticsReadback;
use world::bindings::{
    create_fast_world_bind_group, create_world_bind_group, rebuild_world_bind_groups,
    sync_auto4_bind_groups,
};
use world::detail_textures::{
    auto_detail_fallback_index, build_auto_detail_by_base_texture, detail_texture_for_source,
    required_auto_detail_indices, AUTO_DETAIL_ENABLED_BIT, AUTO_DETAIL_TEXTURE_FILES,
};
use world::draw::{
    average_skybox_color, claim_ocean_clipmap, draw_world_batch, inspector_index_range,
    ocean_suppresses_batch,
};
use world::geometry::{
    build_draw_compaction_groups, make_cull_record, posed_inline_vertices, same_draw_state,
};
#[cfg(test)]
use world::inline_models::plan_inline_runs;
use world::inline_models::{
    inline_run_fog_depth, inline_stage_info, pack_inline_draws, InlineDrawScratch,
};
use world::pipelines::{
    create_depth_prepass_mask_pipeline, create_depth_prepass_pipeline,
    create_entity_prepass_pipeline, create_entity_shadow_pipeline, create_hiz_prepass_pipeline,
    create_legacy_dlight_pass_pipelines, create_legacy_fog_pass_pipelines,
    create_reflection_fog_pass_pipelines, create_reflection_pipelines,
    create_world_pipeline_variant_from_plan, create_world_pipelines, legacy_dlight_receives,
    legacy_fog_pass_jobs, material_uniform, ocean_pipeline_key, same_world_surface,
};
use world::types::{
    GpuReflectionProbe, InlineModelGpu, LegacyDlightRun, PlanarReflectionResources,
    PlanarReflectionView, PlanarReflector, PlanarReflectorKind, ReflectionProbeGpuSet,
    StaticLightGridGpu, UniformArena, UniformSlot, VoxelProbeGiGpu, WorldBatch, WorldBatchSet,
    WorldDrawGroup, WorldGpu, WorldPipelineCompilePlan, WorldPipelineVariant,
    STATIC_AO_ADAPTIVE_MAX_CENTER_ERROR, STATIC_AO_ADAPTIVE_MAX_VISIBILITY_RANGE,
    STATIC_AO_ADAPTIVE_MIN_NORMAL_DOT, STATIC_AO_BVH_LEAF_TRIANGLES, STATIC_AO_HQ_BROAD_RADIUS,
    STATIC_AO_HQ_BROAD_WEIGHT, STATIC_AO_HQ_CONTACT_RADIUS, STATIC_AO_HQ_CONTACT_WEIGHT,
    STATIC_AO_HQ_MEDIUM_RADIUS, STATIC_AO_HQ_MEDIUM_WEIGHT, STATIC_AO_LIGHTMAP_BIAS,
    STATIC_AO_VERTEX_BIAS,
};
use world::upload::build_world;
use world::variants::{WorldRenderPath, WorldShaderFamily, WorldShaderVariantKey};

#[path = "renderer/lighting/rt_models.rs"]
mod rt_models;
#[path = "renderer/lighting/rt_resolution.rs"]
mod rt_resolution;
#[cfg(test)]
#[path = "renderer/tests/shader_audit.rs"]
mod shader_audit;

use crate::{
    camera::{late_latch_third_person_view, Camera, ThirdPersonLateLatchView},
    color_lut,
    fx::draw::FxBlend,
    grass::{GrassDrawStats, GrassMapGpu, GrassPreparedDraw, GrassRenderer},
    jump_shade::JumpShadeState,
    materials::{BlendFactor, BlendFunc, CullMode, TextureData},
    pipeline_jobs::{hash_config as pipeline_hash, PipelineJobKey, PipelineJobManager},
    runtime::{
        RenderStats, ScreenshotOutput, SurfaceInspectorInfo, SurfaceInspectorSection, UserEvent,
        WorldUploadTimings,
    },
    scene::{
        self, BlendMode, DirectionalSun, DrawBatch, DrawClass, GpuVertex, PipelineKey, PreparedMap,
        PreparedPortalDrawPlan, PreparedPortalPlanBatchRef,
    },
    surface_deformation::{
        SurfaceDeformationGpu, SurfaceDeformationStamp, SNOW_SHELL_CHUNK_SIZE,
        SNOW_SHELL_DRAW_RADIUS, SNOW_SHELL_TESSELLATION,
    },
    ui::{
        self, CloudRenderResolution, CloudType, ColorLutPreset, CullDebugMode, DetailTextureMode,
        DofQuality, DynamicLightsMode, DynamicShadowsMode, EntityAmbientLightingMode,
        EntityShadowLight, FogMode, FootprintMode, Ghoul2BatchMode, Ghoul2SkinningMode,
        PlanarReflectionDebugMode, PlanarReflectionMode, PuddleQuality, PvsMode, RainIntensity,
        ReflectionQuality, RendererBackend, SunVisibilityMode, TextureFilter, UiSnapshot, UiVertex,
        VsyncMode,
    },
    weather::{self, FroxelUniform, WeatherOcclusionCache, WeatherSurfaceUniform, WeatherSystem},
};
use bytemuck::{Pod, Zeroable};
use futures_lite::future::block_on;
use glam::{Mat4, Vec2, Vec3, Vec4};
use jka_assets::{
    bsp::MATERIAL_SNOW,
    pk3::AssetSearchPath,
    shader::{AlphaGen, RgbGen, TcGen, TcMod},
};
use std::{
    collections::{hash_map::Entry, BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
        Arc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, event_loop::EventLoopProxy, window::Window};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const UI_BUFFER_BYTES: u64 = 4 * 1024 * 1024;
// Hot gameplay HUD and short-lived chat/center-print geometry are isolated from
// the retained UI. These buffers are intentionally small; a normal frame uses
// only a few hundred vertices.
const UI_DYNAMIC_BUFFER_BYTES: u64 = 512 * 1024;
const UI_TRANSIENT_BUFFER_BYTES: u64 = 1024 * 1024;
const SCREEN_FX_BUFFER_BYTES: u64 = 64 * 1024;
// AUTO physically collapses non-contiguous per-cluster recipes lazily. The GPU
// index buffer reserves at most this much extra space; LRU eviction keeps custom
// maps with thousands of clusters from multiplying index memory without bound.
const AUTO4_COLLAPSE_CACHE_BYTES: u64 = 64 * 1024 * 1024;
const AUTO4_COLLAPSE_QUEUE_DEPTH: usize = 64;
const CLUSTER_X: u32 = 16;
const CLUSTER_Y: u32 = 9;
const CLUSTER_Z: u32 = 24;
const CLUSTER_COUNT: u32 = CLUSTER_X * CLUSTER_Y * CLUSTER_Z;
const MAX_CLUSTER_LIGHTS: u32 = 32;
const MAX_DYNAMIC_LIGHTS: usize = 256;
const MAX_LOCAL_SHADOW_LIGHTS: usize = 4;
const LOCAL_SHADOW_CACHE_SLOTS: usize = 8;
const LOCAL_SHADOW_MAP_SIZE: u32 = 512;
/// `camera_water_surface` / `PostUniform::underwater` value for a camera that is not submerged.
const NO_WATER_SURFACE: f32 = -1.0e30;
const LOCAL_SHADOW_NEAR: f32 = 4.0;
const LOCAL_SHADOW_RESELECT_DISTANCE: f32 = 192.0;
const LOCAL_SHADOW_HYSTERESIS: f32 = 1.35;
const SHADOW_CASCADES: usize = 4;
const SHADOW_MAP_SIZE: u32 = 2048;
// BSP compilers keep only the faces visible from playable space, so a roof or
// ceiling is very often a single face that points down into the room. From the
// sun's side that face is a back face: culling it would let sunlight straight
// through the map's own geometry. Sun casters therefore draw both sides.
const SHADOW_CASTER_CULL: Option<wgpu::Face> = None;
// Bevy 0.19.1 CascadeShadowConfig::default() uses 0.1 / 10.0 / 150.0.
// This renderer already treats 64 JKA units as one metre for weather/world effects,
// so only the unit conversion is applied here; the Bevy cascade ratios are unchanged.
const JKA_UNITS_PER_METER: f32 = 64.0;
const BEVY_CSM_MINIMUM_DISTANCE: f32 = 0.1 * JKA_UNITS_PER_METER;
const BEVY_CSM_FIRST_FAR_BOUND: f32 = 10.0 * JKA_UNITS_PER_METER;
const BEVY_CSM_MAXIMUM_DISTANCE: f32 = 150.0 * JKA_UNITS_PER_METER;
const BEVY_CSM_OVERLAP_PROPORTION: f32 = 0.20;
const BEVY_CSM_SHADOW_DEPTH_BIAS: f32 = 0.02;
const BEVY_CSM_SHADOW_NORMAL_BIAS: f32 = 1.8;
// Bevy Solari samples directional-light visibility across a finite cone. JKA's
// authored sun has no angular-radius field, so RT Shadows uses Earth's measured
// apparent solar diameter as its default emitter extent. The shader receives
// cos(half-angle), matching Bevy's DirectionalLight representation.
const RT_SUN_ANGULAR_DIAMETER_RADIANS: f32 = 0.00930842;
// Dynamic RT casters are deliberately bounded. Static BSP plus inline movers
// occupy fixed slots first; ordinary rigid entities use the remaining tail.
// This avoids allowing an effects-heavy custom server to turn one shadow mode
// into an unbounded TLAS build.
const RT_DYNAMIC_TLAS_INSTANCE_BUDGET: usize = 1024;
const RT_RIGID_BLAS_CACHE_LIMIT: usize = 2048;
const RT_SKINNED_BLAS_CACHE_LIMIT: usize = 2048;
/// Side of the cloud march's interleave block: one texel in grid x grid is
/// marched per frame. Shared by the post uniform and the march viewport.
const CLOUD_INTERLEAVE_GRID: u32 = 2;
const FALLBACK_SUN_DIRECTION: [f32; 3] = [-0.38, -0.84, -0.39];
const FALLBACK_SUN_COLOR: [f32; 3] = [0.57735026, 0.57735026, 0.57735026];
const FALLBACK_SUN_INTENSITY: f32 = 250.0;
const PLANAR_AUTHORED_SCALE_DIVISOR: u32 = 2;
const PLANAR_ENVIRONMENT_SCALE_DIVISOR: u32 = 4;
const PLANAR_REFLECTION_SLOTS: usize = 4;
const PLANAR_REFLECTION_HYSTERESIS: f32 = 1.15;
/// Cloud shape now comes from `cloud_noise`: a tileable Perlin-Worley weather
/// map plus a tileable Worley detail volume, matching the reference the cloud
/// renderer is ported from. The old single 96^3 value-noise volume baked four
/// FBM octaves into one texture, but only the lowest two were above Nyquist at
/// that resolution, so the "high detail" channels contributed aliasing rather
/// than shape.
use crate::cloud_noise;
use crate::cloud_wind;

fn camera_depth_clear() -> f32 {
    0.0
}

fn camera_depth_compare() -> wgpu::CompareFunction {
    wgpu::CompareFunction::GreaterEqual
}

/// Depth bias for shaders that author `polygonOffset` (decals, caustic overlays
/// and other surfaces lying coplanar on a wall).
///
/// The engine calls `glPolygonOffset(-1, -2)` with a forward-Z depth buffer,
/// which pulls the surface toward the camera. The camera depth here is
/// reversed-Z (near -> 1, far -> 0) in a Depth32Float buffer, so "toward the
/// camera" is a POSITIVE bias, and a float buffer's bias unit is far finer than
/// a 24-bit fixed-point unit, so it takes many units to cover the difference in
/// rasterised depth between two coplanar triangles. With the sign wrong the
/// overlay lost the depth test to the wall and only survived in triangle-shaped
/// patches where rounding happened to favour it.
fn polygon_offset_depth_bias() -> wgpu::DepthBiasState {
    debug_assert_eq!(camera_depth_compare(), wgpu::CompareFunction::GreaterEqual);
    wgpu::DepthBiasState {
        constant: 16,
        slope_scale: 1.0,
        clamp: 0.0,
    }
}
const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x2,
    2 => Float32x2,
    5 => Float32x3,
    3 => Float32x4,
    4 => Float32
];
const LEGACY_DLIGHT_SURFACE_ID_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    wgpu::vertex_attr_array![6 => Uint32];
const UI_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32x2,
    2 => Float32x4,
    3 => Float32
];

fn append_vignette_vertices(vertices: &mut Vec<UiVertex>) {
    // Approximate the old smooth radial post-process with a few interpolated
    // rings. The centre is an actual hole, so the rasterizer does no vignette
    // work there. 32 segments keeps the zero-alpha boundary visually round.
    const SEGMENTS: usize = 32;
    const RADII_NDC: [f32; 5] = [0.56, 0.80, 1.04, 1.28, 1.56];

    let alpha_at_radius = |radius_ndc: f32| {
        let radius_uv = radius_ndc * 0.5;
        let t = ((radius_uv - 0.28) / (0.78 - 0.28)).clamp(0.0, 1.0);
        let edge = t * t * (3.0 - 2.0 * t);
        edge * 0.28
    };
    let make_vertex = |radius: f32, angle: f32| UiVertex {
        position: [angle.cos() * radius, angle.sin() * radius],
        uv: [0.0, 0.0],
        color: [0.0, 0.0, 0.0, alpha_at_radius(radius)],
        textured: 0.0,
    };

    for ring in RADII_NDC.windows(2) {
        let inner = ring[0];
        let outer = ring[1];
        for segment in 0..SEGMENTS {
            let a0 = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            let a1 = std::f32::consts::TAU * (segment + 1) as f32 / SEGMENTS as f32;
            let i0 = make_vertex(inner, a0);
            let i1 = make_vertex(inner, a1);
            let o0 = make_vertex(outer, a0);
            let o1 = make_vertex(outer, a1);
            vertices.extend_from_slice(&[i0, o0, o1, i0, o1, i1]);
        }
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

struct Renderer {
    model_frame_log: crate::model_frame_log::ModelFrameLog,
    base: PathBuf,
    /// Retained so auxiliary surfaces can query capabilities against the same GPU adapter.
    adapter: wgpu::Adapter,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline_jobs: PipelineJobManager,
    config: wgpu::SurfaceConfiguration,
    ray_tracing_supported: bool,
    ray_traced_shadows: Option<RayTracedShadowResources>,
    size: PhysicalSize<u32>,
    present_modes: Vec<wgpu::PresentMode>,
    /// Set by [`Renderer::request_scene_rebuild`], consumed once per frame.
    scene_rebuild_pending: bool,
    supported_msaa: Vec<u32>,
    hdr_supported_msaa: Vec<u32>,
    vsync: VsyncMode,
    msaa_samples: u32,
    targets: RenderTargets,
    camera_buffer: wgpu::Buffer,
    camera_layout: wgpu::BindGroupLayout,
    camera_bind_group: wgpu::BindGroup,
    fast_camera_layout: wgpu::BindGroupLayout,
    fast_camera_bind_group: wgpu::BindGroup,
    companion_scene_targets: HashMap<CompanionId, CompanionSceneTarget>,
    companion_scene_generation: u64,
    companion_scene_next_sequence: u64,
    companion_scene_ready: Vec<CompanionSceneFrame>,
    planar_camera_buffers: Vec<wgpu::Buffer>,
    planar_camera_bind_groups: Vec<wgpu::BindGroup>,
    sky_portal_camera_buffer: wgpu::Buffer,
    sky_portal_camera_bind_group: wgpu::BindGroup,
    surface_layout: wgpu::BindGroupLayout,
    fast_surface_layout: wgpu::BindGroupLayout,
    surface_deformation: SurfaceDeformationGpu,
    planar_reflection_layout: wgpu::BindGroupLayout,
    planar_reflection: PlanarReflectionResources,
    weather: WeatherSystem,
    grass_renderer: GrassRenderer,
    surface_sprite_effect_renderer: SurfaceSpriteEffectRenderer,
    ocean_layout: wgpu::BindGroupLayout,
    ocean_optics_layout: wgpu::BindGroupLayout,
    ocean_optics: crate::ocean::optics::Resources,
    ocean_inert_bind_group: wgpu::BindGroup,
    ocean_spray_albedo: GpuImage,
    detail_texture: GpuImage,
    detail_texture_auto: bool,
    detail_texture_game: Option<PathBuf>,
    detail_auto_textures: HashMap<String, GpuImage>,
    ocean: Option<crate::ocean::OceanGpu>,
    authored_oceans: Vec<(
        crate::ocean::authoring::AuthoredOcean,
        crate::ocean::OceanGpu,
    )>,
    /// Shared by every ocean; lazy, see ensure_lazy_scene_pipelines.
    ocean_spray_pipeline: Option<wgpu::RenderPipeline>,
    authored_ocean_definitions: Vec<crate::ocean::authoring::AuthoredOcean>,
    /// Render-space min/max of every water volume this frame (at most eight).
    water_boxes: Vec<([f32; 3], [f32; 3])>,
    /// Top of the water volume the camera is inside, or NO_WATER_SURFACE.
    camera_water_surface: f32,
    ocean_enabled: bool,
    ocean_settings: crate::ocean::OceanSettings,
    world_pipeline_layout: wgpu::PipelineLayout,
    world_pipeline_layout_lean: wgpu::PipelineLayout,
    fast_world_pipeline_layout: wgpu::PipelineLayout,
    world_shader: wgpu::ShaderModule,
    // Advanced path without the newer area/GI bindings. The genuinely minimal
    // known-fast renderer below is a separate shader/layout/render loop.
    world_shader_lean: wgpu::ShaderModule,
    fast_world_shader: wgpu::ShaderModule,
    wireframe_supported: bool,
    wireframe_mask: u32,
    pvs_mode: PvsMode,
    wireframe_pipeline_layout: wgpu::PipelineLayout,
    wireframe_shader: wgpu::ShaderModule,
    wireframe_pipeline: Option<wgpu::RenderPipeline>,
    world_wireframe_pipelines: BTreeMap<WorldShaderVariantKey, wgpu::RenderPipeline>,
    fast_world_wireframe_pipeline: Option<wgpu::RenderPipeline>,
    surface_inspector_shader: wgpu::ShaderModule,
    surface_inspector_pipeline: Option<wgpu::RenderPipeline>,
    debug_volumes: DebugVolumeRenderer,
    debug_volume_shader: wgpu::ShaderModule,
    inspector_vertex_range: Option<std::ops::Range<u32>>,
    inspector_entity_num: Option<u16>,
    /// Scratch for the per-frame blob shadow projection (no allocation once warm).
    blob_mark_buffer: jka_assets::bsp::MarkBuffer,
    ui_pipeline: wgpu::RenderPipeline,
    screen_fx_shader: wgpu::ShaderModule,
    screen_fx_texture_layout: wgpu::BindGroupLayout,
    screen_fx_pipeline_layout: wgpu::PipelineLayout,
    screen_fx_sampler: wgpu::Sampler,
    screen_fx_white_bind_group: wgpu::BindGroup,
    screen_fx_pipelines: HashMap<FxBlend, wgpu::RenderPipeline>,
    screen_fx_textures: HashMap<String, ScreenFxTexture>,
    screen_fx_vertex_buffer: wgpu::Buffer,
    screen_fx_vertex_count: u32,
    screen_fx_batches: Vec<ScreenFxBatch>,
    ui_texture_layout: wgpu::BindGroupLayout,
    ui_bind_group: wgpu::BindGroup,
    _ui_font: GpuImage,
    _ui_small_font: GpuImage,
    _ui_splash: GpuImage,
    /// TaystJK `gfx/hud/keys/*` art packed into one atlas (UI texture source 4).
    _ui_keys: GpuImage,
    /// The 27-image movement key atlas is read only once cg_movementKeys is on;
    /// until then `_ui_keys` is a 1x1 placeholder nothing samples.
    ui_keys_loaded: bool,
    /// `gfx/2d/lag` and `gfx/2d/net` (the lagometer frame and phone jack) side by side
    /// (UI texture source 5), read the first time something draws them; until then a
    /// 1x1 placeholder nothing samples.
    _ui_icons: GpuImage,
    ui_icons_loaded: bool,
    /// Team overlay model/weapon/powerup art packed on demand (UI texture source 6).
    _ui_team_icons: GpuImage,
    ui_team_icon_key: Vec<String>,
    /// True while bind group binding 3 holds the unknown-map art rather than the
    /// splash/levelshot, so a rebuild for another binding keeps the right image.
    ui_binding_is_unknown_map: bool,
    /// OpenJK MP fallback (`menu/art/unknownmap_mp`) uploaded once with the
    /// renderer so entering a load screen never waits on filesystem/image I/O.
    _ui_unknown_map: GpuImage,
    ui_unknown_map_size: Option<[u32; 2]>,
    ui_splash_size: Option<[u32; 2]>,
    ui_loading_image_key: Option<String>,
    ui_pending_levelshot: Option<PendingUiLevelshot>,
    ui_small_font_metrics: Option<ui::ProportionalFont>,
    _ui_font_sampler: wgpu::Sampler,
    ui_vertex_buffer: wgpu::Buffer,
    ui_vertex_count: u32,
    ui_dynamic_vertex_buffer: wgpu::Buffer,
    ui_dynamic_vertex_count: u32,
    ui_dynamic_stable_vertex_count: usize,
    ui_dynamic_vertices: Vec<UiVertex>,
    ui_transient_vertex_buffer: wgpu::Buffer,
    ui_transient_vertex_count: u32,
    ui_transient_stable_vertex_count: usize,
    ui_transient_vertices: Vec<UiVertex>,
    player_names: Option<ui::UiWorldPlayerNames>,
    ui_trace: bool,
    ui_state: UiSnapshot,
    egui_renderer: egui_wgpu::Renderer,
    egui_paint_jobs: Vec<egui::ClippedPrimitive>,
    egui_pixels_per_point: f32,
    egui_active: bool,
    egui_pending_free: Vec<egui::TextureId>,
    asset_preview_mode: bool,
    asset_preview_viewport: Option<[u32; 4]>,
    // Created only after opening a menu page that requests the live blurred/
    // desaturated backdrop. Normal gameplay and the VIDEO setup tab do not
    // compile or allocate this pass.
    menu_backdrop: Option<MenuBackdropResources>,
    world: Option<WorldGpu>,
    /// `videoMap` cinematics of the current world, streamed into `world.textures`.
    world_videos: Vec<WorldVideoPlayer>,
    dynamic_model_renderer: DynamicModelRenderer,
    white: GpuImage,
    missing: GpuImage,
    flat_normal: GpuImage,
    repeat_sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    pbr_repeat_sampler: wgpu::Sampler,
    pbr_clamp_sampler: wgpu::Sampler,
    lightmap_sampler: wgpu::Sampler,
    sky_sampler: wgpu::Sampler,
    texture_filter: TextureFilter,
    /// Latched `r_picmip`. Existing worlds retain their uploaded textures;
    /// the value is consumed the next time `load_map` builds a WorldGpu.
    picmip: u32,
    detail_textures_mode: DetailTextureMode,
    jump_shade: JumpShadeState,
    detail_texture_fade: bool,
    detail_texture_fade_distance: f32,
    gamma: f32,
    baked_brightness: brightness::BakedBrightness,
    gamma_method: crate::gamma::GammaMethod,
    hdr_enabled: bool,
    tone_mapping_enabled: bool,
    auto_exposure_enabled: bool,
    bloom_enabled: bool,
    halation_enabled: bool,
    ssao_enabled: bool,
    static_bsp_ao_enabled: bool,
    static_bsp_ao_lightmap: bool,
    static_bsp_ao_samples: u32,
    static_bsp_ao_resolution: u32,
    static_bsp_ao_strength: u32,
    static_bsp_ao_range: u32,
    static_bsp_ao_current_cell: bool,
    static_ao_pending: Option<StaticAoPendingJob>,
    static_ao_deferred_force_rebuild: bool,
    fxaa_enabled: bool,
    smaa_enabled: bool,
    taa_enabled: bool,
    contact_shadows_enabled: bool,
    sun_override: bool,
    sun_yaw: f32,
    sun_pitch: f32,
    sun_intensity: f32,
    sun_color: [f32; 3],
    sun_visibility: SunVisibilityMode,
    entity_sun_lighting: bool,
    clouds_enabled: bool,
    cloud_type: CloudType,
    cloud_quality: f32,
    cloud_coverage: f32,
    cloud_height: f32,
    cloud_thickness: f32,
    weather_wind: crate::ocean::OceanWind,
    cloud_shadows_enabled: bool,
    cloud_render_resolution: CloudRenderResolution,
    cloud_temporal_enabled: bool,
    cloud_temporal_depth_fix: bool,
    cloud_shear: f32,
    cloud_base_variation: f32,
    cloud_shape_evolution: bool,
    cloud_terrain_interaction: bool,
    cloud_empty_skip: bool,
    cloud_aerial: f32,
    cloud_sky_ambient_enabled: bool,
    cloud_history_blend: f32,
    cloud_motion_reject: f32,
    cloud_history_depth_reject: bool,
    cloud_thickness_variation: f32,
    cloud_size: f32,
    /// Linear average of the current map's skybox faces, used to tint cloud
    /// ambient so a desert or night sky does not light clouds cold blue.
    cloud_sky_average: [f32; 3],
    grass_enabled: bool,
    grass_precompute_enabled: bool,
    grass_mid_lod_enabled: bool,
    grass_front_to_back_enabled: bool,
    contact_shadow_debug: u8,
    cloud_history_valid: bool,
    cloud_history_key: Option<CloudHistoryKey>,
    cloud_history_read_index: usize,
    cloud_temporal_frame_index: u32,
    reflection_quality: ReflectionQuality,
    reflection_debug_enabled: bool,
    ssr_enabled: bool,
    chromatic_aberration_strength: f32,
    vignette_enabled: bool,
    film_grain_strength: f32,
    motion_blur_strength: f32,
    depth_of_field_strength: f32,
    dof_quality: DofQuality,
    dof_focus_distance: f32,
    dof_focus_valid: bool,
    motion_blur_runtime_scale: f32,
    previous_frame_time: f32,
    color_lut_preset: ColorLutPreset,
    color_lut_strength: f32,
    split_toning: crate::color_grading::SplitToningSettings,
    color_lut_effective_strength: f32,
    _color_lut_texture: wgpu::Texture,
    color_lut_view: wgpu::TextureView,
    color_lut_sampler: wgpu::Sampler,
    cloud_noise_resources: Option<CloudNoiseResources>,
    cloud_noise_fallback: CloudNoiseResources,
    frame_plan: FramePlan,
    /// Benchmark control: never select `WorldRenderPath::FastBaseline`.
    force_unified_world: bool,
    indirect_supported: bool,
    gpu_compaction_supported: bool,
    compact_group_scratch: Vec<bool>,
    // CPU-culling counterpart to the GPU compactor. These retained vectors let
    // the ordinary PVS + frustum path pack same-state visible draws into the
    // existing indirect buffer without per-frame allocation churn.
    cpu_compact_indirect_scratch: Vec<DrawIndexedIndirectArgs>,
    cpu_compact_count_scratch: Vec<u32>,
    reflection_compact_scratch: Vec<DrawIndexedIndirectArgs>,
    /// Per compact group for the current reflection slot: (first packed
    /// entry, entry count). A drawn group's count is reset to 0.
    reflection_compact_groups: Vec<(u32, u32)>,
    /// Retained scratch for grouping visible mover batches into multi-draws.
    inline_visible_scratch: Vec<usize>,
    /// Visible transparent BSP stages deferred until promoted Godot water has
    /// written its displaced depth. Stores positions in the already-selected
    /// active draw list so no second visibility/PVS traversal is required.
    post_ocean_transparent_scratch: Vec<usize>,
    inline_pose_scratch: Vec<Option<InlineModelInstance>>,
    inline_draw_scratch: InlineDrawScratch,
    gpu_driven_enabled: bool,
    hiz_occlusion_enabled: bool,
    dynamic_lights_mode: DynamicLightsMode,
    rt_samples: u32,
    dynamic_light_falloff: u32,
    rt_reduced_shadows: bool,
    clustered_lighting_enabled: bool,
    map_light_simulation_enabled: bool,
    classic_fullbright: bool,
    classic_vertex_light: bool,
    classic_lightmap_only: bool,
    transient_lights: Vec<TransientLight>,
    cloud_foreground: Vec<CloudForegroundBlade>,
    /// Setup > Video brightness multiplier applied to runtime dynamic lights.
    dynamic_light_brightness: f32,
    emissive_area_lights_enabled: bool,
    irradiance_volume_enabled: bool,
    voxel_probe_gi_enabled: bool,
    local_light_shadows_enabled: bool,
    /// Inside a settings batch: `activate_world_pipeline_variant` only records
    /// that it is owed, so intermediate setting combinations never compile.
    settings_batch_open: bool,
    variant_activation_pending: bool,
    /// Desired specialized world variant while its render pipelines compile on
    /// the shared worker pool. The current hot variant remains active meanwhile.
    world_variant_pending: Option<WorldShaderVariantKey>,
    pbr_enabled: bool,
    /// Parallax occlusion on top of PBR (`r_pom`). Only meaningful while PBR is on.
    pom_enabled: bool,
    deluxe_mapping_enabled: bool,
    deluxe_specular: f32,
    bc_compression_supported: bool,
    cascaded_shadows_enabled: bool,
    cascaded_shadow_mode: DynamicShadowsMode,
    entity_shadow: EntityShadowState,
    entity_shadow_light: EntityShadowLight,
    cull_debug_mode: CullDebugMode,
    planar_reflection_mode: PlanarReflectionMode,
    planar_reflection_debug_mode: PlanarReflectionDebugMode,
    last_planar_debug_selection: Option<Vec<usize>>,
    planar_slot_history: [Option<[f32; 4]>; PLANAR_REFLECTION_SLOTS],
    planar_debug_pipeline: wgpu::RenderPipeline,
    history_valid: bool,
    camera_history_valid: bool,
    previous_view_proj: Mat4,
    previous_unjittered_view_proj: Mat4,
    previous_cloud_view_proj: Mat4,
    previous_camera_position: Vec3,
    taa_frame_index: u32,
    ssao_history_valid: bool,
    ssao_history_read_index: usize,
    ssr_history_valid: bool,
    ssr_history_read_index: usize,
    ssr_frame_index: u32,
    depth_prepass_pipeline: wgpu::RenderPipeline,
    depth_prepass_mask_pipeline: wgpu::RenderPipeline,
    /// Depth-only world pipelines for the Hi-Z early pass (no colour targets,
    /// so none of the full prepass's motion / reflection-policy fill cost).
    hiz_prepass_pipeline: wgpu::RenderPipeline,
    hiz_prepass_mask_pipeline: wgpu::RenderPipeline,
    hiz_build_layout: wgpu::BindGroupLayout,
    hiz_build_pipeline: wgpu::ComputePipeline,
    hiz_build_bind_group: wgpu::BindGroup,
    hiz_reduce_layout: wgpu::BindGroupLayout,
    hiz_reduce_pipeline: wgpu::ComputePipeline,
    hiz_reduce_bind_groups: Vec<wgpu::BindGroup>,
    gpu_cull_layout: wgpu::BindGroupLayout,
    gpu_cull_pipeline: wgpu::ComputePipeline,
    gpu_cull_early_pipeline: wgpu::ComputePipeline,
    gpu_cull_settings_buffer: wgpu::Buffer,
    cull_debug_layout: wgpu::BindGroupLayout,
    cull_debug_pipeline_layout: wgpu::PipelineLayout,
    cull_debug_shader: wgpu::ShaderModule,
    cull_debug_pipeline: Option<wgpu::RenderPipeline>,
    cluster_compute_layout: wgpu::BindGroupLayout,
    cluster_compute_pipeline: wgpu::ComputePipeline,
    lighting_layout: wgpu::BindGroupLayout,
    lighting_layout_lean: wgpu::BindGroupLayout,
    shadow_receiver_layout: wgpu::BindGroupLayout,
    shadow_caster_layout: wgpu::BindGroupLayout,
    lighting_settings_buffer: wgpu::Buffer,
    pbr_settings_buffer: wgpu::Buffer,
    shadow_resources: ShadowResources,
    auto_exposure_layout: wgpu::BindGroupLayout,
    auto_exposure_pipeline: wgpu::ComputePipeline,
    auto_exposure_state_buffer: wgpu::Buffer,
    auto_exposure_settings_buffer: wgpu::Buffer,
    auto_exposure_bind_group: wgpu::BindGroup,
    post_layout: wgpu::BindGroupLayout,
    post_shader: wgpu::ShaderModule,
    post_pipeline_layout: wgpu::PipelineLayout,
    /// `None` until the background compile finishes and a frame needs it; see
    /// `ensure_post_pipeline`.
    post_pipeline: Option<wgpu::RenderPipeline>,
    taa_post_pipeline: Option<wgpu::RenderPipeline>,
    cloud_march_pipeline: wgpu::RenderPipeline,
    cloud_resolve_pipeline: wgpu::RenderPipeline,
    cloud_resolve_layout: wgpu::BindGroupLayout,
    cloud_resolve_bind_group: wgpu::BindGroup,
    post_sampler: wgpu::Sampler,
    post_buffer: wgpu::Buffer,
    post_bind_groups: [[[[wgpu::BindGroup; 2]; 2]; 2]; 2],
    history_read_index: usize,
    ssao_temporal_layout: wgpu::BindGroupLayout,
    ssao_temporal_pipeline: wgpu::RenderPipeline,
    ssao_temporal_buffer: wgpu::Buffer,
    ssao_temporal_bind_groups: Option<[wgpu::BindGroup; 2]>,
    ssr_temporal_layout: wgpu::BindGroupLayout,
    ssr_temporal_pipeline: wgpu::RenderPipeline,
    ssr_temporal_buffer: wgpu::Buffer,
    ssr_temporal_bind_groups: Option<[wgpu::BindGroup; 2]>,
    _ssr_visibility_layout: Option<wgpu::BindGroupLayout>,
    ssr_visibility_pipeline: Option<wgpu::RenderPipeline>,
    ssr_visibility_bind_group: Option<wgpu::BindGroup>,
    bloom_layout: wgpu::BindGroupLayout,
    bloom_extract_pipeline: wgpu::RenderPipeline,
    bloom_downsample_pipeline: wgpu::RenderPipeline,
    bloom_bind_groups: Option<[wgpu::BindGroup; 3]>,
    dof_layout: wgpu::BindGroupLayout,
    dof_pipeline: wgpu::RenderPipeline,
    dof_buffer: wgpu::Buffer,
    dof_bind_group: Option<wgpu::BindGroup>,
    gamma_post_layout: wgpu::BindGroupLayout,
    gamma_post_pipeline: wgpu::RenderPipeline,
    gamma_post_buffer: wgpu::Buffer,
    gamma_post_bind_group: wgpu::BindGroup,
    fast_post_layout: wgpu::BindGroupLayout,
    fast_post_pipeline: wgpu::RenderPipeline,
    fast_post_bind_group: wgpu::BindGroup,
    // Created lazily on first use so SMAA adds no resources or pipeline work
    // to the maximum-FPS path unless the user selects it.
    smaa_target: Option<smaa::SmaaTarget>,
    gpu_profiler: GpuProfiler,
    cull_diagnostics_readback: CullDiagnosticsReadback,
    surface_diag_pending: bool,
    frame_diag_pending: bool,
    screenshot_supported: bool,
    screenshot_requested: Option<ScreenshotDestination>,
    screenshot_result: Option<Result<ScreenshotOutput, String>>,
    screenshot_readback_buffer: Option<ScreenshotReadbackBuffer>,
    screenshot_worker_tx: Sender<ScreenshotEncodeJob>,
    screenshot_worker_rx: Receiver<Result<ScreenshotOutput, String>>,
    started: Instant,
}
#[cfg(test)]
#[path = "renderer/tests/sun_shadow_tests.rs"]
mod sun_shadow_tests;

#[cfg(test)]
#[path = "renderer/tests/inline_run_tests.rs"]
mod inline_run_tests;

#[cfg(test)]
#[path = "renderer/tests/shader_tests.rs"]
mod shader_tests;

#[cfg(test)]
#[path = "renderer/tests/entity_shadow_tests.rs"]
mod entity_shadow_tests;
