//! Bounded, opt-in CPU frame-encoding capture. No draw-site locks, allocations,
//! WGPU readbacks, device polls, or per-frame disk IO. Frame functions and model
//! draw phases each read one relaxed boolean while unarmed. File writing runs after capture on a
//! separate OS thread, never in the render/present critical section.
use crate::renderer::diagnostics::profiling::{GpuPass, GPU_PASS_COUNT};
use crate::renderer::{active_batch_selection, FrameInfo, Renderer};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static CAPTURE_ENABLED: AtomicBool = AtomicBool::new(false);
// Separately trace only the first map frame. Do not arm a performance capture,
// write any files, or affect the existing capture's ownership of its flag.
static FIRST_WORLD_FRAME_TRACE: AtomicBool = AtomicBool::new(false);

pub(in crate::renderer) fn set_first_world_frame_trace(enabled: bool) {
    FIRST_WORLD_FRAME_TRACE.store(enabled, Ordering::Relaxed);
}

pub(crate) fn first_world_frame_trace() -> bool {
    FIRST_WORLD_FRAME_TRACE.load(Ordering::Relaxed)
}

pub(in crate::renderer) fn enabled() -> bool {
    CAPTURE_ENABLED.load(Ordering::Relaxed)
        || FIRST_WORLD_FRAME_TRACE.load(Ordering::Relaxed)
}

// Deliberately preserve the original inclusive CPU timing fields next to the
// new *non-overlapping* intervals so existing diagnostics are not redefined.
const FIELDS: [&str; 225] = [
    "frame_ms",
    "cpu_prepare_ms",
    "cpu_acquire_ms",
    "cpu_encode_ms",
    "encode_setup_ms",
    "encode_pre_world_ms",
    "encode_scene_ms",
    "encode_post_ui_ms",
    "encoder_finish_ms",
    "profiler_readback_encode_ms",
    "encode_other_ms",
    "primary_submit_api_ms",
    "followup_ms",
    "cpu_submit_legacy_ms",
    "present_ms",
    "gpu_latest_available_ms",
    "world_pvs_batches",
    "world_encoded_batches",
    "world_multidraw_groups",
    "world_multidraw_batches",
    "world_sort_candidates",
    "world_sort_runs",
    "world_sort_moved",
    "world_sort_theoretical_binds_before",
    "world_sort_theoretical_binds_after",
    "world_shader_mode_requested",
    "world_shader_mode_active",
    "world_general_program_binds",
    "world_specialized_program_binds",
    "world_minimal_program_binds",
    "world_shared_program_binds",
    "world_generated_program_binds",
    "primary_command_buffers",
    "frame_inline_render_passes_calls",
    "frame_inline_compute_passes_calls",
    "frame_inline_set_pipeline_calls",
    "frame_inline_set_bind_group_calls",
    "frame_inline_set_vertex_buffer_calls",
    "frame_inline_set_index_buffer_calls",
    "frame_inline_draw_calls",
    "frame_inline_draw_indexed_calls",
    "frame_inline_world_helper_draw_indexed_calls",
    "frame_inline_draw_indexed_indirect_calls",
    "frame_inline_multi_draw_indexed_indirect_calls",
    "frame_inline_multi_draw_indexed_indirect_count_calls",
    "deferred_world_bundle_hit",
    "deferred_world_bundle_build",
    "deferred_world_bundle_draws_represented",
    "world_cpu_compaction_active",
    "world_cpu_compact_visible_members",
    "world_cpu_compact_singleton_groups",
    "world_cpu_compact_multistage_excluded_batches",
    "world_face_plane_rejected_batches",
    "world_face_plane_partial_batches",
    "world_face_plane_rejected_triangles",
    "world_face_plane_direct_draws",
    "ghoul2_prepared_instances",
    "ghoul2_prepared_draws",
    "cpu_model_prepared_ranges",
    "gpu_fx_prepared_ranges",
    "model_phase_cpu_draw_indexed_calls",
    "model_phase_fx_draw_calls",
    "model_phase_ghoul2_draw_indexed_calls",
    "model_phase_set_pipeline_calls",
    "model_phase_set_bind_group_calls",
    "model_phase_set_vertex_buffer_calls",
    "model_phase_set_index_buffer_calls",
    "dynamic_model_prepare_ms",
    "dynamic_vertex_stream_upload_bytes",
    "dynamic_index_stream_upload_bytes",
    "gpu_fx_instance_upload_bytes",
    "ghoul2_bone_upload_bytes",
    "ghoul2_draw_upload_bytes",
    "model_gather_ms",
    "model_cpu_pack_ms",
    "model_ghoul2_group_ms",
    "model_upload_and_pipeline_ms",
    "model_pack_parallel_used",
    "model_index_upload_skipped_bytes",
    "ghoul2_instanced_groups",
    "ghoul2_singleton_opaque_draws",
    "ghoul2_ordered_instances",
    "ghoul2_companion_excluded_instances",
    "ghoul2_largest_instance_group",
    "ghoul2_batch_key_cache_hits",
    "ghoul2_batch_key_cache_misses",
    "cpu_md3_surfaces",
    "cpu_md3_vertices",
    "cpu_md3_indices",
    "cpu_ghoul2_surfaces",
    "cpu_ghoul2_vertices",
    "cpu_ghoul2_indices",
    "cpu_mesh_fx_surfaces",
    "cpu_mesh_fx_vertices",
    "cpu_mesh_fx_indices",
    "cpu_blob_shadow_surfaces",
    "cpu_blob_shadow_vertices",
    "cpu_blob_shadow_indices",
    "cpu_other_surfaces",
    "cpu_other_vertices",
    "cpu_other_indices",
    "gpu_latest_world_ms",
    "gpu_latest_depth_ms",
    "gpu_latest_post_ms",
    "gpu_latest_models_opaque_ms",
    "gpu_latest_models_translucent_ms",
    "gpu_latest_fx_opaque_ms",
    "gpu_latest_fx_translucent_ms",
    "gpu_latest_rt_build_ms",
    "gpu_latest_grass_ms",
    "gpu_latest_clouds_ms",
    "gpu_latest_ocean_sim_ms",
    "gpu_latest_coastal_sim_ms",
    "gpu_latest_ocean_surface_ms",
    "gpu_latest_ocean_optics_ms",
    "gpu_latest_blue_water_underwater_ms",
    "lamp_shadow_ab_mode",
    "gpu_latest_lamp_static_ms",
    "gpu_latest_lamp_moving_ms",
    "cpu_lamp_moving_encode_ms",
    "lamp_dynamic_faces",
    "lamp_entity_face_pairs",
    "lamp_bsp_face_pairs",
    "lamp_static_cache_rebuilds",
    "ocean_backend",
    "ocean_original_surfaces",
    "ocean_triangles",
    "ocean_native_prepare_ms",
    "ocean_native_mesh_build_ms",
    "ocean_native_mesh_upload_ms",
    "ocean_native_mesh_rebuilds",
    "ocean_native_mesh_vertices",
    "ocean_native_mesh_indices",
    "ocean_native_mesh_upload_bytes",
    "ocean_adaptive_mesh_mode",
    "sky_backend",
    "cloud_backend",
    "gpu_latest_blue_water_sky_prepare_ms",
    "gpu_latest_blue_water_sky_draw_ms",
    "gpu_latest_blue_water_clouds_ms",
    "gpu_latest_blue_water_aerial_ms",
    "gpu_latest_blue_water_caustics_compute_ms",
    "gpu_latest_blue_water_caustics_receiver_ms",
    "gpu_latest_blue_water_scene_capture_ms",
    "blue_water_mesh_segments",
    "blue_water_mesh_levels",
    "client_latest_total_ms",
    "client_latest_snapshot_ms",
    "client_latest_entity_present_ms",
    "client_latest_player_present_ms",
    "client_latest_fx_ms",
    "client_latest_fx_tessellate_ms",
    "client_latest_ghoul2_pose_ms",
    "client_latest_ghoul2_skin_ms",
    "client_latest_ghoul2_bolt_ms",
    "client_latest_fx_cpu_geom_surfaces",
    "client_latest_fx_cpu_vertices",
    "client_latest_fx_cpu_indices",
    "client_latest_fx_gpu_sprite_batches",
    "client_latest_fx_gpu_sprite_instances",
    "md3_geometry_cache_hits",
    "md3_geometry_cache_misses",
    "md3_geometry_reused_vertices",
    "md3_vertex_upload_skipped_bytes",
    "md3_index_upload_skipped_bytes",
    "model_bundle_key_ms",
    "model_bundle_update_ms",
    "model_bundle_cache_hits",
    "model_bundle_cache_misses",
    "model_bundle_builds",
    "model_bundle_candidate_draws",
    "model_phase_execute_bundles_calls",
    "model_phase_bundle_indexed_draws",
    "encoder_finish_split_active",
    "encoder_finish_prefix_ms",
    "encoder_finish_tail_ms",
    "encoder_finish_dispatch_ms",
    "encoder_finish_join_wait_ms",
    "world_main_stage_batches",
    "world_main_stage_indices",
    "world_main_stage_triangle_equivalents",
    "world_main_distinct_surfaces",
    "world_main_distinct_indices",
    "world_main_inline_stage_batches",
    "world_main_sky_stage_batches",
    "world_main_largest_stage_indices",
    "world_main_visibility_cpu_culled",
    "world_main_opaque_stage_triangles",
    "world_main_mask_stage_triangles",
    "world_main_transparent_stage_triangles",
    "world_main_sky_stage_triangles",
    "world_main_glow_replay_stages_noninline",
    "world_main_glow_replay_triangles_noninline",
    "inline_models_active",
    "inline_models_hidden",
    "inline_models_uninitialized",
    "inline_hidden_stage_batches_skipped",
    "inline_hidden_stage_triangle_equivalents_skipped",
    "patch_visible_count",
    "patch_full_triangles",
    "patch_selected_triangles",
    "patch_lod_updates",
    "patch_index_upload_bytes",
    "patch_cpu_lod_ms",
    "world_variant_pending",
    "world_active_pipelines",
    "world_active_specialized_pipelines",
    "bsp_primary_stats_shader_mode_latest",
    "bsp_primary_vertex_invocations_latest",
    "bsp_primary_clipped_primitives_latest",
    "bsp_primary_fragment_invocations_latest",
    "ocean_surface_vertex_invocations_latest",
    "ocean_surface_clipped_primitives_latest",
    "ocean_surface_fragment_invocations_latest",
    "ocean_samples_passed_latest",
    "loop_top_ms",
    "loop_command_drain_ms",
    "loop_mailbox_ms",
    "loop_pre_render_ms",
    "loop_pre_scene_rebuild_ms",
    "loop_pre_inline_models_ms",
    "plan_linear_depth_reasons",
    "loop_render_call_ms",
    "loop_post_render_prev_ms",
    "frame_interval_ms",
];

/// Render-thread loop sections around `Renderer::render()`, measured only
/// while a capture is armed. Post-render time belongs to the previous loop
/// iteration (it ends where this one starts). All in ms; NaN when not measured.
#[derive(Clone, Copy)]
pub(in crate::renderer) struct LoopSections {
    /// Loop start to command drain: HDR polling, GPU-timing toggles, device check.
    pub(in crate::renderer) top_ms: f64,
    /// Deferred map uploads plus the render-command drain.
    pub(in crate::renderer) drain_ms: f64,
    /// Static-AO polling, snapshot mailbox clone and HUD/name/FX setters.
    pub(in crate::renderer) mailbox_ms: f64,
    /// Scene rebuild, inline models, companion targets, FPS graph, camera/input sampling.
    pub(in crate::renderer) pre_render_ms: f64,
    /// Parts of `pre_render_ms`: pending scene rebuild, and inline-model posing/upload.
    pub(in crate::renderer) pre_scene_rebuild_ms: f64,
    pub(in crate::renderer) pre_inline_models_ms: f64,
    /// The `render()` call itself.
    pub(in crate::renderer) render_call_ms: f64,
    /// Previous iteration: after `render()` returned until the next loop start.
    pub(in crate::renderer) post_render_prev_ms: f64,
}

impl Default for LoopSections {
    fn default() -> Self {
        Self {
            top_ms: f64::NAN,
            drain_ms: f64::NAN,
            mailbox_ms: f64::NAN,
            pre_render_ms: f64::NAN,
            pre_scene_rebuild_ms: f64::NAN,
            pre_inline_models_ms: f64::NAN,
            render_call_ms: f64::NAN,
            post_render_prev_ms: f64::NAN,
        }
    }
}

#[derive(Clone)]
struct PerfRow {
    path: &'static str,
    numbers: [f64; FIELDS.len()],
    /// Every `GpuPass` from the newest resolved readback, written after FIELDS
    /// as `gpu_pass_<label>_ms` (NaN when that pass did not run or no timing).
    gpu_passes: [f64; GPU_PASS_COUNT],
}

impl PerfRow {
    fn from_frame(info: &FrameInfo, renderer: &Renderer, client: &crate::renderer::ClientFramePerf, sections: LoopSections) -> Self {
        let models = &renderer.dynamic_model_renderer;
        let model_calls = models.perf_phase_calls.get();
        let prepare = &models.perf_prepare;
        let gpu_timing = renderer.gpu_profiler.enabled()
            .then(|| renderer.gpu_profiler.latest_timing()).flatten();
        // Asynchronous (about 10 Hz) hardware counts for the primary BSP pass only.
        let bsp = renderer.frame_time_graph.latest_primary_bsp_stats();
        // Same cadence, the OceanSurface interval. FS / samples_passed > 1 is
        // shading spent on helper lanes and late-rejected fragments.
        let ocean = renderer.frame_time_graph.latest_ocean_surface_stats();
        // Vertex/index bytes come from actual queue writes. Other retained
        // streams are full writes; guard empty-input frames against stale scratch.
        let has_input = info.dynamic_model_surfaces != 0;
        fn bytes<T>(values: &[T], has_input: bool) -> f64 {
            if has_input { std::mem::size_of_val(values) as f64 } else { 0.0 }
        }
        let categorized_encode = info.perf_encode_setup_ms
            + info.perf_encode_pre_world_ms
            + info.perf_encode_scene_ms
            + info.perf_encode_post_ui_ms
            + info.perf_encoder_finish_ms
            + info.perf_profiler_readback_encode_ms;
        Self {
            path: info.world_path,
            numbers: [
                info.frame_ms,
                info.cpu_prepare_ms,
                info.cpu_acquire_ms,
                info.cpu_encode_ms,
                info.perf_encode_setup_ms,
                info.perf_encode_pre_world_ms,
                info.perf_encode_scene_ms,
                info.perf_encode_post_ui_ms,
                info.perf_encoder_finish_ms,
                info.perf_profiler_readback_encode_ms,
                (info.cpu_encode_ms - categorized_encode).max(0.0),
                info.perf_primary_submit_api_ms,
                info.perf_followup_ms,
                info.cpu_submit_ms,
                info.cpu_present_ms,
                info.gpu_ms.unwrap_or(f64::NAN),
                f64::from(info.world_pvs_batches),
                f64::from(info.world_encoded_batches),
                f64::from(info.world_multidraw_groups),
                f64::from(info.world_multidraw_batches),
                f64::from(info.world_sort_candidates),
                f64::from(info.world_sort_runs),
                f64::from(info.world_sort_moved),
                f64::from(info.world_sort_binds_before),
                f64::from(info.world_sort_binds_after),
                f64::from(info.world_shader_programs.requested),
                f64::from(info.world_shader_programs.active),
                f64::from(info.world_shader_programs.binds[0]),
                f64::from(info.world_shader_programs.binds[1]),
                f64::from(info.world_shader_programs.binds[2]),
                f64::from(info.world_shader_programs.binds[3]),
                f64::from(info.world_shader_programs.binds[4]),
                f64::from(info.perf_primary_command_buffers),
                f64::from(info.perf_encode_calls.render_passes),
                f64::from(info.perf_encode_calls.compute_passes),
                f64::from(info.perf_encode_calls.set_pipeline),
                f64::from(info.perf_encode_calls.set_bind_group),
                f64::from(info.perf_encode_calls.set_vertex_buffer),
                f64::from(info.perf_encode_calls.set_index_buffer),
                f64::from(info.perf_encode_calls.draw),
                f64::from(info.perf_encode_calls.draw_indexed),
                f64::from(info.perf_encode_calls.world_helper_draw_indexed),
                f64::from(info.perf_encode_calls.draw_indexed_indirect),
                f64::from(info.perf_encode_calls.multi_draw_indexed_indirect),
                f64::from(info.perf_encode_calls.multi_draw_indexed_indirect_count),
                f64::from(info.perf_deferred_bundle_hit),
                f64::from(info.perf_deferred_bundle_build),
                f64::from(info.perf_deferred_bundle_draws),
                f64::from(u8::from(info.world_cpu_compaction_active)),
                f64::from(info.world_cpu_compact_visible_members),
                f64::from(info.world_cpu_compact_singleton_groups),
                f64::from(info.world_cpu_compact_multistage_excluded_batches),
                f64::from(info.world_face_plane_rejected_batches),
                f64::from(info.world_face_plane_partial_batches),
                f64::from(info.world_face_plane_rejected_triangles),
                f64::from(info.world_face_plane_direct_draws),
                f64::from(info.ghoul2_gpu_instances),
                f64::from(info.ghoul2_gpu_draw_calls),
                f64::from(models.draws.len() as u32),
                f64::from(models.fx_sprite_draws.len() as u32),
                f64::from(model_calls.cpu_draw_indexed),
                f64::from(model_calls.fx_draw),
                f64::from(model_calls.skin_draw_indexed),
                f64::from(model_calls.set_pipeline),
                f64::from(model_calls.set_bind_group),
                f64::from(model_calls.set_vertex_buffer),
                f64::from(model_calls.set_index_buffer),
                info.dynamic_model_prepare_ms,
                prepare.vertex_upload_bytes as f64,
                prepare.index_upload_bytes as f64,
                bytes(models.scratch_fx_sprite_instances.as_slice(), has_input),
                bytes(models.scratch_bones.as_slice(), has_input),
                bytes(models.scratch_draw_data.as_slice(), has_input),
                prepare.gather_ms,
                prepare.cpu_pack_ms,
                prepare.ghoul2_group_ms,
                prepare.upload_ms,
                u8::from(prepare.parallel_used) as f64,
                prepare.index_skipped_bytes as f64,
                prepare.ghoul2_instanced_groups as f64,
                prepare.ghoul2_singleton_groups as f64,
                prepare.ghoul2_ordered_instances as f64,
                prepare.ghoul2_excluded_instances as f64,
                prepare.ghoul2_largest_group as f64,
                prepare.batch_key_hits as f64,
                prepare.batch_key_misses as f64,
                prepare.cpu_sources[0].surfaces as f64,
                prepare.cpu_sources[0].vertices as f64,
                prepare.cpu_sources[0].indices as f64,
                prepare.cpu_sources[1].surfaces as f64,
                prepare.cpu_sources[1].vertices as f64,
                prepare.cpu_sources[1].indices as f64,
                prepare.cpu_sources[2].surfaces as f64,
                prepare.cpu_sources[2].vertices as f64,
                prepare.cpu_sources[2].indices as f64,
                prepare.cpu_sources[3].surfaces as f64,
                prepare.cpu_sources[3].vertices as f64,
                prepare.cpu_sources[3].indices as f64,
                prepare.cpu_sources[4].surfaces as f64,
                prepare.cpu_sources[4].vertices as f64,
                prepare.cpu_sources[4].indices as f64,
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::World.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::Depth.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::Post.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::DynamicModels.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::DynamicModelsTranslucent.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::FxSpritesOpaque.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::FxSpritesTranslucent.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::RtBuild.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::Grass.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::Clouds.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::OceanSim.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::CoastalSim.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::OceanSurface.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::OceanOptics.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::BlueWaterUnderwater.index()]).unwrap_or(f64::NAN),
                f64::from(renderer.lamp_shadow_ab),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::LocalShadowStatic.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::LocalShadowMoving.index()]).unwrap_or(f64::NAN),
                renderer.world.as_ref().map_or(f64::NAN, |world| world.local_shadows.ab_last_cpu_ms),
                renderer.world.as_ref().map_or(f64::NAN, |world| f64::from(world.local_shadows.ab_last_dynamic_faces)),
                renderer.world.as_ref().map_or(f64::NAN, |world| f64::from(world.local_shadows.ab_last_entity_face_pairs)),
                renderer.world.as_ref().map_or(f64::NAN, |world| f64::from(world.local_shadows.ab_last_bsp_face_pairs)),
                renderer.world.as_ref().map_or(f64::NAN, |world| f64::from(world.local_shadows.ab_last_static_rebuilds)),
                f64::from(renderer.ocean_settings.backend),
                f64::from(info.ocean.original_surfaces),
                info.ocean.triangles as f64,
                info.ocean.native_cpu.prepare_ms,
                info.ocean.native_cpu.build_ms,
                info.ocean.native_cpu.upload_ms,
                f64::from(info.ocean.native_cpu.rebuilds),
                info.ocean.native_cpu.vertices as f64,
                info.ocean.native_cpu.indices as f64,
                info.ocean.native_cpu.upload_bytes as f64,
                f64::from(renderer.ocean_settings.adaptive_mesh),
                f64::from(renderer.sky_backend),
                f64::from(renderer.cloud_backend),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::BlueWaterSkyPrepare.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::BlueWaterSkyDraw.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::BlueWaterClouds.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::BlueWaterAerial.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::BlueWaterCausticsCompute.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::BlueWaterCausticsReceiver.index()]).unwrap_or(f64::NAN),
                gpu_timing.and_then(|timing| timing.milliseconds[crate::renderer::GpuPass::BlueWaterSceneCapture.index()]).unwrap_or(f64::NAN),
                renderer.ocean_settings.blue_water_mesh_segments as f64,
                f64::from(renderer.ocean_settings.blue_water_mesh_levels),
                client.total_ms,
                client.snapshot_ms,
                client.entity_present_ms,
                client.player_present_ms,
                client.fx_ms,
                client.fx_tessellate_ms,
                client.ghoul2_pose_ms,
                client.ghoul2_skin_ms,
                client.ghoul2_bolt_ms,
                client.fx_cpu_geom_surfaces as f64,
                client.fx_cpu_vertices as f64,
                client.fx_cpu_indices as f64,
                client.fx_gpu_sprite_batches as f64,
                client.fx_gpu_sprite_instances as f64,
                prepare.md3_cache_hits as f64,
                prepare.md3_cache_misses as f64,
                prepare.md3_reused_vertices as f64,
                prepare.md3_vertex_skipped_bytes as f64,
                prepare.md3_index_skipped_bytes as f64,
                models.perf_bundles.key_ms,
                models.perf_bundles.build_ms,
                models.perf_bundles.hits as f64,
                models.perf_bundles.misses as f64,
                models.perf_bundles.builds as f64,
                models.perf_bundles.eligible_draws as f64,
                model_calls.bundle_executes as f64,
                model_calls.bundle_draw_indexed as f64,
                u8::from(info.perf_encoder_split_active) as f64,
                info.perf_encoder_prefix_finish_ms,
                info.perf_encoder_tail_finish_ms,
                info.perf_encoder_dispatch_ms,
                info.perf_encoder_join_wait_ms,
                info.perf_world_stage_batches as f64,
                info.perf_world_stage_indices as f64,
                info.perf_world_stage_indices as f64 / 3.0,
                info.perf_world_distinct_surfaces as f64,
                info.perf_world_distinct_indices as f64,
                info.perf_world_inline_stages as f64,
                info.perf_world_sky_stages as f64,
                info.perf_world_largest_stage_indices as f64,
                u8::from(info.perf_world_cpu_visibility) as f64,
                info.perf_world_opaque_indices as f64 / 3.0,
                info.perf_world_mask_indices as f64 / 3.0,
                info.perf_world_transparent_indices as f64 / 3.0,
                info.perf_world_sky_indices as f64 / 3.0,
                info.perf_world_glow_replay_stages as f64,
                info.perf_world_glow_replay_indices as f64 / 3.0,
                info.perf_inline_models_active as f64,
                info.perf_inline_models_hidden as f64,
                info.perf_inline_models_uninitialized as f64,
                info.perf_inline_hidden_stage_batches as f64,
                info.perf_inline_hidden_stage_indices as f64 / 3.0,
                info.patch_lod.visible_patches as f64,
                info.patch_lod.full_triangles as f64,
                info.patch_lod.selected_triangles as f64,
                info.patch_lod.lod_updates as f64,
                info.patch_lod.index_upload_bytes as f64,
                info.patch_lod.cpu_lod_ms,
                f64::from(u8::from(info.world_shader_programs.pending)),
                f64::from(info.world_pipeline_counts.0),
                f64::from(info.world_pipeline_counts.1),
                bsp.map_or(f64::NAN, |stats| f64::from(stats.shader_mode)),
                bsp.map_or(f64::NAN, |stats| stats.vertex_invocations as f64),
                bsp.map_or(f64::NAN, |stats| stats.clipped_primitives as f64),
                bsp.map_or(f64::NAN, |stats| stats.fragment_invocations as f64),
                ocean.map_or(f64::NAN, |stats| stats.vertex_invocations as f64),
                ocean.map_or(f64::NAN, |stats| stats.clipped_primitives as f64),
                ocean.map_or(f64::NAN, |stats| stats.fragment_invocations as f64),
                ocean.map_or(f64::NAN, |_| renderer.ocean_visibility.latest_samples as f64),
                sections.top_ms,
                sections.drain_ms,
                sections.mailbox_ms,
                sections.pre_render_ms,
                sections.pre_scene_rebuild_ms,
                sections.pre_inline_models_ms,
                f64::from(renderer.frame_plan.linear_depth_reasons),
                sections.render_call_ms,
                sections.post_render_prev_ms,
                f64::NAN,
            ],
            gpu_passes: std::array::from_fn(|pass| {
                gpu_timing.and_then(|timing| timing.milliseconds[pass]).unwrap_or(f64::NAN)
            }),
        }
    }
}

pub(in crate::renderer) struct PerfCapture {
    stem: PathBuf,
    armed_at: Instant,
    frames: Vec<PerfRow>,
    target_frames: usize,
    /// Time-bounded capture: wall time measured from the first recorded frame.
    target_duration: Option<Duration>,
    recording_started: Option<Instant>,
    recording_wall: Option<Duration>,
    /// Successive record() calls: the real presented-frame cadence, unlike
    /// frame_ms which only spans the renderer's own frame function.
    last_record: Option<Instant>,
    warmup_frames: usize,
    warmup_remaining: usize,
    hooks_owned: bool,
    adapter: String,
    driver: String,
    backend: String,
    fingerprint: String,
    /// Map-load classification, NOT a measured per-material GPU cost.
    material_audit: serde_json::Value,
    /// Capture-only, indexed by the *stable* loaded material debug ID.
    pvs_candidate_names: Vec<String>,
    pvs_candidate_stages: Vec<usize>,
    pvs_candidate_batches: Vec<u64>,
    pvs_candidate_indices: Vec<u64>,
    /// Post-CPU-selection main camera stages, accumulated only over sampled frames.
    main_view_stages: Vec<crate::renderer::api::WorldStagePerf>,
}

impl PerfCapture {
    pub(in crate::renderer) fn arm(stem: PathBuf, frames: usize, seconds: Option<f64>, renderer: &Renderer) -> Self {
        // Ten seconds at 2-3k FPS exceeds the frame-count limit; time-bounded
        // captures keep a generous safety cap (~1.6 KB per row).
        let target_frames = if seconds.is_some() { 200_000 } else { frames.clamp(30, 6_000) };
        let adapter = renderer.adapter.get_info();
        let warmup_frames = 120;
        CAPTURE_ENABLED.store(true, Ordering::Relaxed);
        Self {
            stem,
            armed_at: Instant::now(),
            frames: Vec::with_capacity(target_frames.min(16_384)),
            target_frames,
            target_duration: seconds.map(Duration::from_secs_f64),
            recording_started: None,
            recording_wall: None,
            last_record: None,
            warmup_frames,
            warmup_remaining: warmup_frames,
            hooks_owned: true,
            adapter: adapter.name,
            driver: format!("{} {}", adapter.driver, adapter.driver_info),
            backend: format!("{:?}", adapter.backend),
            material_audit: material_audit(renderer),
            pvs_candidate_names: renderer.world.as_ref().map(|w| w.material_debug.entries.iter().map(|e| e.name.clone()).collect()).unwrap_or_default(),
            pvs_candidate_stages: renderer.world.as_ref().map(|w| w.material_debug.entries.iter().map(|e| e.resolved_stage_count).collect()).unwrap_or_default(),
            pvs_candidate_batches: vec![0; renderer.world.as_ref().map_or(0, |w| w.material_debug.entries.len())],
            pvs_candidate_indices: vec![0; renderer.world.as_ref().map_or(0, |w| w.material_debug.entries.len())],
            main_view_stages: vec![crate::renderer::api::WorldStagePerf::default(); renderer.world.as_ref().map_or(0, |w| w.material_debug.entries.len())],
            fingerprint: format!(
                "{}x{};present={:?};latency={};msaa={};gpu_timestamps={};pvs={:?};world={:?};ghoul2_batch={};model_state_cache={};dynamic_batch={};world_compact_min_draws={};fx_zero_alpha_discard={};gpu_driven={};hiz={};legacy_drawfog={};dynamic_lights={:?};model_prepare_threads={};model_index_upload_cache={};ghoul2_batch_key_cache={};md3_geometry_cache={};ocean_enabled={};ocean_effective={};model_render_bundles={};encoder_finish_threads={};world_stage_probe={};fastsky={};inline_models={};inline_transform={};bsp_model_surfaces={};early_submit={};shader_mode={};world_sort={};active_world={};hdr_output={};present_copy={}",
                renderer.config.width,
                renderer.config.height,
                renderer.config.present_mode,
                renderer.config.desired_maximum_frame_latency,
                renderer.msaa_samples,
                renderer.gpu_profiler.enabled(),
                renderer.pvs_mode,
                renderer.frame_plan.world_path,
                renderer.dynamic_model_renderer.ghoul2_batch_draws.config_value(),
                renderer.dynamic_model_renderer.model_state_cache,
                renderer.dynamic_model_renderer.dynamic_batch_draws,
                renderer.world_compact_min_draws,
                renderer.dynamic_model_renderer.fx_zero_alpha_discard,
                renderer.gpu_driven_enabled,
                renderer.hiz_occlusion_enabled,
                renderer.weather.fog.legacy_drawfog_value(),
                renderer.dynamic_lights_mode,
                renderer.dynamic_model_renderer.model_prepare_threads,
                renderer.dynamic_model_renderer.model_index_upload_cache,
                renderer.dynamic_model_renderer.ghoul2_batch_key_cache,
                renderer.dynamic_model_renderer.md3_geometry_cache,
                renderer.ocean_enabled,
                renderer.ocean_enabled && renderer.world.as_ref().is_some_and(|w| w.active_pipeline_variant.ocean),
                renderer.dynamic_model_renderer.model_render_bundles,
                renderer.encoder_finish_threads,
                renderer.world_stage_probe,
                renderer.fast_sky,
                renderer.draw_inline_models,
                renderer.inline_transform_mode.config_value(),
                renderer.draw_bsp_model_surfaces,
                renderer.early_submit_enabled,
                renderer.fragment_shader_mode,
                renderer.world_sort_mode,
                renderer.world.as_ref().map(|w| w.active_pipeline_variant.short_label()).unwrap_or_default(),
                renderer.hdr_output.as_ref().map_or(0, |h| h.requested),
                renderer.present_copy_enabled,
            ),
        }
    }

    /// Called only after successfully presented frames. Wait two seconds so
    /// users can close the console; then skip 120 additional successful frames
    /// to avoid measuring the first dynamic resource/path warmup. This does not
    /// pretend pipelines are fully prewarmed: repeat the scene before capture.
    pub(in crate::renderer) fn record(&mut self, info: &FrameInfo, renderer: &Renderer, client: &crate::renderer::ClientFramePerf, sections: LoopSections) -> bool {
        if self.armed_at.elapsed() < Duration::from_secs(2)
            || info.perf_primary_command_buffers == 0
        {
            return false;
        }
        if self.warmup_remaining != 0 {
            self.warmup_remaining -= 1;
            return false;
        }
        // PVS candidates, deliberately not "draws": GPU culling, areaportals,
        // frustum, reflection passes, occlusion, water and fog can reject or
        // replay these stages. Aggregated off the render hot path only while a
        // bounded capture is active; no allocation or per-draw locks.
        if let Some(world) = renderer.world.as_ref() {
            if world.material_debug.entries.len() == self.pvs_candidate_batches.len() {
                let active = active_batch_selection(world, renderer.pvs_mode);
                for &index in active.indices {
                    let Some(batch) = active.batches.get(index) else { continue; };
                    let Ok(id) = usize::try_from(batch.source.material_debug_index) else { continue; };
                    if let (Some(batches), Some(indices)) = (
                        self.pvs_candidate_batches.get_mut(id),
                        self.pvs_candidate_indices.get_mut(id),
                    ) {
                        *batches = batches.saturating_add(1);
                        *indices = indices.saturating_add(u64::from(
                            batch.indexed_range.end.saturating_sub(batch.indexed_range.start),
                        ));
                    }
                }
            }
        }
        if self.main_view_stages.len() == info.perf_world_material_stages.len() {
            for (total, frame) in self.main_view_stages.iter_mut().zip(&info.perf_world_material_stages) {
                total.stages = total.stages.saturating_add(frame.stages);
                total.stage_indices = total.stage_indices.saturating_add(frame.stage_indices);
                total.resolved_stage_equivalents = total.resolved_stage_equivalents.saturating_add(frame.resolved_stage_equivalents);
                total.glow_replay_draws = total.glow_replay_draws.saturating_add(frame.glow_replay_draws);
                total.distinct_surfaces = total.distinct_surfaces.saturating_add(frame.distinct_surfaces);
                total.distinct_indices = total.distinct_indices.saturating_add(frame.distinct_indices);
                total.inline_stages = total.inline_stages.saturating_add(frame.inline_stages);
                total.skipped_hidden_stages = total.skipped_hidden_stages.saturating_add(frame.skipped_hidden_stages);
                total.skipped_hidden_indices = total.skipped_hidden_indices.saturating_add(frame.skipped_hidden_indices);
            }
        }
        let now = Instant::now();
        let started = *self.recording_started.get_or_insert(now);
        let mut row = PerfRow::from_frame(info, renderer, client, sections);
        if let Some(previous) = self.last_record.replace(now) {
            row.numbers[FIELDS.len() - 1] = now.duration_since(previous).as_secs_f64() * 1000.0;
        }
        self.frames.push(row);
        let done = self.frames.len() >= self.target_frames
            || self.target_duration.is_some_and(|limit| started.elapsed() >= limit);
        if done {
            self.recording_wall = Some(started.elapsed());
        }
        done
    }

    /// Convert the now-complete bounded capture into an off-render-thread
    /// write. No live GPU objects or `Renderer` borrows cross this boundary.
    pub(in crate::renderer) fn finish(mut self) {
        CAPTURE_ENABLED.store(false, Ordering::Relaxed);
        self.hooks_owned = false;
        std::thread::Builder::new().name("jka-renderperf-write".into()).spawn(move || {
            let csv = self.csv();
            let summary = self.summary();
            let csv_path = self.stem.with_extension("csv");
            let json_path = self.stem.with_extension("json");
            let result = (|| -> std::io::Result<()> {
                if let Some(parent) = csv_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&csv_path, csv)?;
                std::fs::write(&json_path, summary)?;
                Ok(())
            })();
            match result {
                Ok(()) => eprintln!(
                    "[RENDER PERF] wrote {} frames to {} and {}",
                    self.frames.len(), csv_path.display(), json_path.display()
                ),
                Err(error) => eprintln!("[RENDER PERF] capture write failed: {error}"),
            }
        }).expect("spawn render perf writer");
    }

    fn csv(&self) -> String {
        let mut out = String::from("frame_index,renderer_path,");
        out.push_str(&FIELDS.join(","));
        for pass in GpuPass::ALL {
            out.push_str(",gpu_pass_");
            out.push_str(pass.label());
            out.push_str("_ms");
        }
        out.push('\n');
        for (index, row) in self.frames.iter().enumerate() {
            use std::fmt::Write;
            let _ = write!(&mut out, "{index},{},", row.path);
            for (field, value) in row.numbers.iter().enumerate() {
                if field != 0 { out.push(','); }
                if value.is_finite() { let _ = write!(&mut out, "{value:.6}"); }
            }
            for value in &row.gpu_passes {
                out.push(',');
                if value.is_finite() { let _ = write!(&mut out, "{value:.6}"); }
            }
            out.push('\n');
        }
        out
    }

    fn sampled_pvs_candidates(&self) -> serde_json::Value {
        let mut rows = self.pvs_candidate_names.iter().enumerate().filter_map(|(id, name)| {
            let batches = self.pvs_candidate_batches.get(id).copied().unwrap_or(0);
            if batches == 0 { return None; }
            Some(serde_json::json!({
                "material": name,
                "resolved_stages": self.pvs_candidate_stages[id],
                "pvs_candidate_batch_occurrences": batches,
                "pvs_candidate_index_occurrences": self.pvs_candidate_indices[id],
            }))
        }).collect::<Vec<_>>();
        rows.sort_unstable_by(|a, b| {
            b["pvs_candidate_index_occurrences"].as_u64().cmp(&a["pvs_candidate_index_occurrences"].as_u64())
        });
        let top20 = rows.iter().filter(|r| r["resolved_stages"].as_u64().unwrap_or(0) > 1)
            .take(20).cloned().collect::<Vec<_>>();
        serde_json::json!({
            "sampled_frames": self.frames.len(),
            "method": "PVS-selected material index/batch occurrences AFTER frame completed; no frustum, area, GPU occlusion, ocean replacement, secondary views, fog passes or GPU work inference",
            "top20_multistage_pvs_candidates": top20,
            "all_materials": rows,
        })
    }

    fn sampled_main_view_geometry(&self) -> serde_json::Value {
        let n = self.frames.len().max(1) as f64;
        let mut rows = self.main_view_stages.iter().enumerate().filter_map(|(id, row)| {
            if row.stages == 0 && row.skipped_hidden_stages == 0 { return None; }
            Some(serde_json::json!({
                "material": self.pvs_candidate_names.get(id),
                "resolved_stages": self.pvs_candidate_stages.get(id),
                "mean_stage_batches_per_frame": row.stages as f64 / n,
                "mean_resolved_stage_equivalents_per_frame": row.resolved_stage_equivalents as f64 / n,
                "mean_stage_draws_saved_by_fusion_per_frame": row.resolved_stage_equivalents.saturating_sub(row.stages) as f64 / n,
                "mean_actual_glow_replay_draws_per_frame": row.glow_replay_draws as f64 / n,
                "mean_stage_triangles_per_frame": row.stage_indices as f64 / (n * 3.0),
                "mean_distinct_surfaces_per_frame": row.distinct_surfaces as f64 / n,
                "mean_distinct_triangles_per_frame": row.distinct_indices as f64 / (n * 3.0),
                "mean_inline_stage_batches_per_frame": row.inline_stages as f64 / n,
                "mean_skipped_hidden_stage_batches_per_frame": row.skipped_hidden_stages as f64 / n,
                "mean_skipped_hidden_triangles_per_frame": row.skipped_hidden_indices as f64 / (n * 3.0),
            }))
        }).collect::<Vec<_>>();
        rows.sort_unstable_by(|a,b| {
            b["mean_stage_triangles_per_frame"].as_f64().unwrap_or(0.0)
                .total_cmp(&a["mean_stage_triangles_per_frame"].as_f64().unwrap_or(0.0))
        });
        let mut hidden = rows.clone();
        hidden.sort_unstable_by(|a, b| {
            b["mean_skipped_hidden_stage_batches_per_frame"].as_f64().unwrap_or(0.0)
                .total_cmp(&a["mean_skipped_hidden_stage_batches_per_frame"].as_f64().unwrap_or(0.0))
        });
        let top20_hidden = hidden.into_iter()
            .filter(|row| row["mean_skipped_hidden_stage_batches_per_frame"].as_f64().unwrap_or(0.0) > 0.0)
            .take(20).collect::<Vec<_>>();
        serde_json::json!({
            "method": "Main-camera prepared world draw-batch candidates after CPU visibility filters; the new stage-equivalent counter includes fused shader stages (2/3 represented by one candidate draw), and their difference measures avoided *candidate* draw submissions. GPU-driven frames include GPU-culled candidates. Authored glow replay draws are actual emitted non-inline glow-pass draw_indexed calls; inline glow batches are not included there. No per-material GPU times, fragment invocations, fog or secondary-view calls. Distinct triangle count uses adjacent-stage equivalence. No additional GPU readbacks.",
            "sampled_frames": self.frames.len(),
            "top20_by_stage_triangles": rows.iter().take(20).cloned().collect::<Vec<_>>(),
            "top20_by_skipped_hidden_stages": top20_hidden,
            "all_materials": rows,
        })
    }

    fn summary(&self) -> String {
        let mut stats = serde_json::Map::new();
        for (index, field) in FIELDS.iter().enumerate() {
            let mut values: Vec<f64> = self.frames.iter()
                .map(|row| row.numbers[index])
                .filter(|value| value.is_finite())
                .collect();
            values.sort_unstable_by(f64::total_cmp);
            stats.insert((*field).to_owned(), if values.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!({
                    "mean": values.iter().sum::<f64>() / values.len() as f64,
                    "p50": percentile(&values, 0.50),
                    "p95": percentile(&values, 0.95),
                    "p99": percentile(&values, 0.99),
                    "min": values[0],
                    "max": values[values.len() - 1],
                    "samples": values.len(),
                })
            });
        }
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "dinurdojk-cpu-renderperf-v10",
            "sample_count": self.frames.len(),
            "requested_seconds": self.target_duration.map(|d| d.as_secs_f64()),
            "recording_wall_seconds": self.recording_wall.map(|d| d.as_secs_f64()),
            "wall_fps": self.recording_wall.filter(|d| !d.is_zero())
                .map(|d| self.frames.len() as f64 / d.as_secs_f64()),
            "warmup_seconds_before_frame_skip": 2,
            "warmup_skipped_frames": self.warmup_frames,
            "adapter": self.adapter,
            "driver": self.driver,
            "backend": self.backend,
            "settings_fingerprint_at_arm": self.fingerprint,
            "material_audit_static_not_gpu_timing": self.material_audit,
            "material_audit_sampled_pvs_candidates_not_gpu_timing": self.sampled_pvs_candidates(),
            "material_audit_main_camera_stage_geometry_not_gpu_timing": self.sampled_main_view_geometry(),
            "caveats": [
                "encode_pre_world includes shadows, RT, reflection, grass and environment work before the main scene, and is not a pure shadow timer",
                "encode_scene includes opaque, transparent, dynamic models, and FX (not distinct buckets)",
                "encode_post_ui includes all post, screenshot encoding and in-main UI work until encoder.finish",
                "gpu_latest_available_ms is an asynchronous profiler result and need not correspond to the same CSV frame",
                "cpu_submit_legacy_ms includes follow-up work; primary_submit_api_ms is the isolated primary queue call",
                "primary_command_buffers counts the principal frame command buffer and optional profiler-readback command buffer ONLY",
                "world_multidraw_groups counts API groups, not executed per-surface indexed draws",
                "world_main_* counts post-CPU-filtered main-camera authored BSP stage index spans, including repeated shader stages; not unique global triangles, pixel count, or GPU timing. GPU-driven frames provide conservative candidates, not actual executed draws. Material table omits fog/glow replay and secondary-view draws",
                "inline_models_* are render-instance slots (not distinct BSP model handles): active=Some(Some(pose)), hidden=Some(None), uninitialized=None. Only explicit hidden slots are skipped on the CPU-driven main camera; GPU-driven visibility retains the conservative collapsed-geometry fallback",
                "inline_hidden_stage_* are PVS-selected CPU main-view stages omitted before area/frustum checks; thus not equivalent to the number of draw calls saved. Additional reflection views also skip explicitly hidden slots but do not contribute to this count",
                "frame_inline_* counters are exact call-site counts for instrumented frame functions, not all nested renderer modules and not executed GPU indirect draw totals",
                "model_phase_* are actual API calls in DynamicModelRenderer::draw_phase, including secondary/glow views; exclude wireframe, shadow helper and compute commands",
                "ghoul2_prepared_* and *_prepared_ranges describe prepared main-view geometry, not actual per-pass calls; cpu_model includes MD3, CPU-skinned models, mesh FX and RT-skinned indexed streams",
                "world_cpu_compact_* audit only the CPU main-view compaction path, not GPU-driven visibility; singleton counts are eligible visible groups before the minimum-size test",
                "*_upload_bytes cover actual dynamic vertex/index writes and full GPU FX instance/Ghoul2 bone/draw streams; exclude cache-miss mesh creation, textures and extra RT metadata/raw-color streams; dynamic streams include MD3 and mesh FX, not MD3-only costs",
                "world_cpu_compact_multistage_excluded_batches counts visible grouped opaque/masked batches rejected due to adjacent sibling stages, not all multistage or transparent geometry",
                "model_gather includes serial CPU lighting/rebasing in threads=0; modes 1/2/4 move that work to model_cpu_pack; compare their sum rather than either interval alone; RT uses its original preparation path",
                "model_upload_and_pipeline includes capacity checks, lazy pipeline readiness, queue writes and optional index comparisons; it is not a pure PCIe transfer timer",
                "cpu_* source counts describe indexed CPU input geometry including clipped blob output, not GPU skin/sprite inputs or actual replay draw counts",
                "ghoul2_singleton_opaque_draws includes opaque/masked draws not instanced by the current batching threshold; ordered and companion-excluded inputs stay conservative",
                "dynamic_vertex_stream_upload_bytes and dynamic_index_stream_upload_bytes record actual queue writes; resident MD3 ranges and the index cache may skip writes; stream sizes exclude cache-miss resource creation and RT metadata",
                "md3_geometry_cache_hits measure exact source/light/offset reuse in raster preparation; source counts still include cached inputs; changed animation/transform/color/light/visibility may cause misses; buffer growth and >32 fragmented dirty spans force full uploads even with CPU hits",
                "model_phase_execute_bundles_calls are API executions, while model_phase_bundle_indexed_draws counts indexed draws represented inside them; add represented draws to model_phase_cpu_draw_indexed_calls when comparing logical indexed-model draw totals",
                "model_bundle_key_ms assembles exact resource/draw keys; model_bundle_update_ms includes equality checks and optional bundle recording, not only cache-miss builds; candidate draws include modes below the 32-draw eligibility threshold; only ordinary opaque/masked raster draws can use bundles",
                "encoder_finish_ms is renderer blocking finalization: original finish in mode 0, prefix+tail serial finishes in mode 1, tail finish+worker join wait in active mode 2; worker prefix duration overlaps other recording/finish and must not be added to renderer frame/encode totals",
                "encoder_finish_prefix_ms is serial-prefix time or worker-task elapsed time; dispatch includes tail-encoder creation/handoff and nests in encode_scene; mode 1 excludes prefix finish from scene to avoid double-counting; split_active reports actual use, since unsupported views stay on the original path",
                "gpu_latest_* use asynchronous resolved pass timings, not matching the CPU CSV frame; model/FX/grass intervals nest inside world and must not be summed with world/frame totals",
                "client_latest_* are telemetry from the consumed client snapshot, may repeat across renderer frames, include overlapping/nested intervals and exclude waits; never add client total to renderer frame time",
                "no image parity or hardware benchmarks are implied by this capture"
            ],
            "metrics": stats,
        })).unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
    }
}

impl Drop for PerfCapture {
    fn drop(&mut self) {
        if self.hooks_owned {
            CAPTURE_ENABLED.store(false, Ordering::Relaxed);
        }
    }
}

/// Count emitted *static map-load* recipes for each material independently
/// for MINIMAL/OFF and FULL PVS. These are separate alternative representations,
/// never add the two column totals or treat them as visible frame draws.
/// World triangles are represented by index spans; GPU-driven compaction,
/// frustum/PVS, fog replays, secondary cameras and ocean suppression all change
/// the executed work and are explicitly excluded from this audit.
fn material_audit(renderer: &Renderer) -> serde_json::Value {
    let Some(world) = renderer.world.as_ref() else {
        return serde_json::json!({ "available": false, "reason": "no map loaded" });
    };
    #[derive(Default)]
    struct Counts {
        coarse_batches: u64,
        coarse_indices: u64,
        full_batches: u64,
        full_indices: u64,
        coarse_inline_batches: u64,
        coarse_grouped_batches: u64,
        full_grouped_batches: u64,
        coarse_lightmap_pages: std::collections::HashSet<Option<usize>>,
        full_lightmap_pages: std::collections::HashSet<Option<usize>>,
    }
    let mut counts: Vec<Counts> = (0..world.material_debug.entries.len())
        .map(|_| Counts::default()).collect();
    for (batches, coarse) in [(&world.coarse_batches, true), (&world.full_batches, false)] {
        for batch in batches.iter() {
            let Ok(index) = usize::try_from(batch.source.material_debug_index) else { continue };
            let Some(count) = counts.get_mut(index) else { continue };
            let indices = u64::from(batch.indexed_range.end.saturating_sub(batch.indexed_range.start));
            if coarse {
                count.coarse_batches += 1;
                count.coarse_indices += indices;
                if batch.is_inline_entity { count.coarse_inline_batches += 1; }
                count.coarse_grouped_batches += u64::from(batch.compact_group.is_some());
                count.coarse_lightmap_pages.insert(batch.source.lightmap);
            } else {
                count.full_batches += 1;
                count.full_indices += indices;
                count.full_grouped_batches += u64::from(batch.compact_group.is_some());
                count.full_lightmap_pages.insert(batch.source.lightmap);
            }
        }
    }
    let mut entries: Vec<_> = world.material_debug.entries.iter().zip(&counts)
        .filter(|(_, counts)| counts.coarse_batches > 0 || counts.full_batches > 0)
        .map(|(entry, count)| {
            serde_json::json!({
                "name": entry.name,
                "source": entry.source,
                "mtr_override": entry.mtr_override,
                "authored_stages": entry.authored_stage_count,
                "resolved_stages": entry.resolved_stage_count,
                "audit_class": entry.stage_audit_class,
                "stage_sequence": entry.stage_sequence,
                "coarse_batches": count.coarse_batches,
                "coarse_triangle_indices": count.coarse_indices,
                "coarse_inline_batches": count.coarse_inline_batches,
                "full_batches": count.full_batches,
                "full_triangle_indices": count.full_indices,
                "coarse_compaction_group_members": count.coarse_grouped_batches,
                "full_compaction_group_members": count.full_grouped_batches,
                "coarse_distinct_lightmap_bindings": count.coarse_lightmap_pages.len(),
                "full_distinct_lightmap_bindings": count.full_lightmap_pages.len(),
            })
        }).collect();
    entries.sort_unstable_by(|a,b| {
        let weight = |entry: &serde_json::Value| {
            (entry["coarse_triangle_indices"].as_u64().unwrap_or(0),
             entry["coarse_batches"].as_u64().unwrap_or(0))
        };
        weight(b).cmp(&weight(a))
    });
    let total = entries.len();
    let offenders: Vec<_> = entries.iter()
        .filter(|v| v["resolved_stages"].as_u64().unwrap_or(0) >= 2)
        .take(20).cloned().collect();
    let groups = world.compact_groups.iter().map(|group| {
        let (set, batch) = match group.representative_set {
            crate::renderer::WorldBatchSet::Coarse =>
                ("coarse", &world.coarse_batches[group.representative_index]),
            crate::renderer::WorldBatchSet::Full =>
                ("full", &world.full_batches[group.representative_index]),
        };
        let material = world.material_debug.entries.get(batch.source.material_debug_index as usize)
            .map(|entry| entry.name.as_str());
        serde_json::json!({ "set": set, "material": material, "map_load_members": group.max_count })
    }).collect::<Vec<_>>();
    serde_json::json!({
        "available": true,
        "method": "map-loaded source draw batches, ranked by coarse triangle-index spans (NOT shader GPU time, visible pixels or true per-frame draw calls)",
        "alternatives": "coarse = OFF/MINIMAL representation; full = FULL/exact PVS representation; do not sum them",
        "ranked_materials_total": total,
        "top20_multistage_static_candidates": offenders,
        "all_materials_by_static_indices": entries,
        "compaction_groups_static_not_visible": groups,
    })
}

fn percentile(values: &[f64], p: f64) -> f64 {
    debug_assert!(!values.is_empty());
    let position = p * (values.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    values[lower] + (values[upper] - values[lower]) * (position - lower as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summary_percentiles_interpolate_sorted_samples() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&values, 0.5), 3.0);
        assert!((percentile(&values, 0.95) - 4.8).abs() < 1e-10);
    }
    #[test]
    fn export_fields_have_unique_names() {
        let mut names = FIELDS.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), FIELDS.len());
    }
}
