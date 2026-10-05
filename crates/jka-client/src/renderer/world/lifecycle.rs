//! World lifecycle.
use crate::renderer::{
    build_auto_detail_by_base_texture, build_world, create_gpu_cull_bind_group,
    create_world_froxel_bind_group, Arc, DetailTextureMode, EntityLegacyFog, GrassMapGpu, Instant,
    Mat4, PreparedMap, Renderer, WorldShaderFamily, WorldUploadTimings, WorldVideoPlayer,
    PBR_PROFILE_COMPANION_COMPRESSION, WORLD_VIDEO_MAX_CATCH_UP,
};

impl Renderer {
    pub(in crate::renderer) fn load_map(&mut self, mut map: PreparedMap) -> WorldUploadTimings {
        let load_map_started = Instant::now();
        // A new world must not inherit transient RE_AddLightToScene submissions
        // from the previous map. The next CGame snapshot will repopulate them.
        self.transient_lights.clear();
        self.debug_volumes.set_map(Arc::clone(&map.debug_volumes));
        // Start the pure-CPU weather heightfield solve immediately so it overlaps
        // GPU/world upload. First rain enable should normally only upload the
        // already-computed field instead of stalling the render thread.
        let weather_occlusion = map.weather_occlusion.take();
        self.weather.rain.start_map_occlusion(weather_occlusion);
        self.inspector_vertex_range = None;
        self.inspector_entity_num = None;
        // Acceleration structures point at the previous world's buffers. Drop
        // them before replacing the world; they are rebuilt lazily below only
        // when RT Shadows is actually selected.
        self.ray_traced_shadows = None;
        self.surface_deformation.clear(&self.queue);
        let footprint_mask = map
            .footprint_mark_textures
            .iter()
            .enumerate()
            .fold(0u32, |mask, (index, texture)| {
                mask | if texture.is_some() { 1u32 << index } else { 0 }
            });
        self.surface_deformation.set_legacy_mark_info(
            &self.queue,
            footprint_mask,
            map.footprint_mark_blend_modes,
        );
        self.history_valid = false;
        self.camera_history_valid = false;
        self.dof_focus_valid = false;
        self.previous_frame_time = 0.0;
        self.previous_unjittered_view_proj = Mat4::IDENTITY;
        self.previous_cloud_view_proj = Mat4::IDENTITY;
        self.cloud_history_valid = false;
        self.cloud_history_read_index = 0;
        self.cloud_temporal_frame_index = 0;
        self.taa_frame_index = 0;
        self.history_read_index = 0;
        self.ssao_history_valid = false;
        self.ssao_history_read_index = 0;
        self.ssr_history_valid = false;
        self.ssr_history_read_index = 0;
        self.ssr_frame_index = 0;
        self.weather.fog.install_map(
            map.global_fog,
            map.batches.iter().map(|batch| {
                (
                    batch.fog,
                    u64::from(batch.vertices.end.saturating_sub(batch.vertices.start)),
                )
            }),
        );
        let source_map = map.map_file_stats.is_some();
        let videos = std::mem::take(&mut map.videos);
        // The BLAS needs the uploaded world buffers, so map upload always creates
        // a normal raster variant first.  If RT Shadows is selected we build the
        // acceleration structures immediately after WorldGpu is installed, then
        // lazily compile/switch to the dedicated RT pipeline variant.
        let mut initial_shader_variant = self.world_shader_variant_key_for_source_map(source_map);
        initial_shader_variant.ray_traced_shadows = false;
        initial_shader_variant.ray_traced_sun = false;
        let detail_texture_by_base = build_auto_detail_by_base_texture(
            map.textures.len(),
            map.batches
                .iter()
                .chain(map.pvs_batches.iter())
                .chain(map.inline_batches.iter()),
        );
        if self.detail_textures_mode != DetailTextureMode::Off {
            self.ensure_auto_detail_textures_for_plan(&detail_texture_by_base);
        }
        let (world_pipeline_layout, world_shader) = match initial_shader_variant.family() {
            WorldShaderFamily::Enhanced => (&self.world_pipeline_layout, &self.world_shader),
            WorldShaderFamily::Lean => (&self.world_pipeline_layout_lean, &self.world_shader_lean),
        };
        let pre_build_ms = load_map_started.elapsed().as_secs_f64() * 1000.0;
        let (mut world, mut upload_timings) = build_world(
            &self.device,
            &self.queue,
            &self.surface_layout,
            &self.fast_surface_layout,
            world_pipeline_layout,
            world_shader,
            initial_shader_variant,
            self.scene_format(),
            self.msaa_samples,
            self.ray_tracing_supported,
            initial_shader_variant.legacy_fog,
            &self.white,
            &self.missing,
            &self.flat_normal,
            &self.repeat_sampler,
            &self.clamp_sampler,
            &self.pbr_repeat_sampler,
            &self.pbr_clamp_sampler,
            &self.lightmap_sampler,
            &self.sky_sampler,
            &self.detail_texture,
            self.detail_texture_auto,
            &self.detail_auto_textures,
            detail_texture_by_base,
            &self.cluster_compute_layout,
            &self.lighting_layout,
            &self.lighting_layout_lean,
            &self.shadow_caster_layout,
            &self.cull_debug_layout,
            &self.lighting_settings_buffer,
            &self.pbr_settings_buffer,
            self.surface_deformation.buffer(),
            self.surface_deformation.field_views(),
            self.surface_deformation.field_sampler(),
            &self.grass_renderer,
            &self.surface_sprite_effect_renderer,
            self.ocean_enabled,
            self.pbr_enabled && PBR_PROFILE_COMPANION_COMPRESSION && self.bc_compression_supported,
            self.picmip,
            map,
        );
        upload_timings.pre_build_ms = pre_build_ms;
        let post_bind_started = Instant::now();
        world.cull_bind_group = Some(create_gpu_cull_bind_group(
            &self.device,
            &self.gpu_cull_layout,
            &world.cull_records_buffer,
            &world.indirect_buffer,
            &self.targets.hiz_view,
            &self.gpu_cull_settings_buffer,
            &world.active_cull_indices_buffer,
            &world.compact_indirect_buffer,
            &world.compact_count_buffer,
            &world.cull_debug_reason_buffer,
            &world.cull_debug_count_buffer,
            &world.early_indirect_buffer,
        ));
        world.froxel_bind_group = Some(create_world_froxel_bind_group(
            &self.device,
            &self.weather.fog.resources.layout,
            &self.weather.fog.resources.uniform_buffer,
            &self.weather.fog.resources.buffer,
            &self.shadow_resources._array_view,
            &self.shadow_resources.sky_array_view,
            &world.light_buffer,
            &world._cluster_buffer,
            &self.lighting_settings_buffer,
            &world.local_shadows.cube_view,
            &world.local_shadows.sampler,
            self.weather.rain.collision_view(),
            &self.weather.rain.gpu.wind_noise_view,
            &self.weather.rain.gpu.wind_noise_sampler,
        ));
        upload_timings.post_bind_groups_ms = post_bind_started.elapsed().as_secs_f64() * 1000.0;
        let post_misc_started = Instant::now();
        self.world = Some(world);
        self.world_videos = videos
            .iter()
            .filter_map(
                |source| match jka_assets::roq::RoqVideo::open(Arc::clone(&source.data)) {
                    Ok(video) => Some(WorldVideoPlayer {
                        texture: source.texture,
                        video,
                        next_frame_at: 0.0,
                    }),
                    Err(error) => {
                        eprintln!("[JKA] video {}: {error}", source.name);
                        None
                    }
                },
            )
            .collect();
        self.sync_baked_brightness_assets();
        if self.hardware_rt_requested() {
            self.ensure_ray_traced_shadow_resources();
        }
        // build_world allocates the fixed-capacity storage with every authored
        // light so the buffer exists before Renderer owns the WorldGpu. Rewrite
        // it immediately through the active source-class filter: source .map
        // simulation excludes lightJunior from direct surface lighting and the
        // normal Forward+/area toggles must also be correct on the first frame.
        self.upload_light_buffer();
        self.request_static_ao(false);
        let sun = self.active_sun();
        self.grass_renderer.update_environment(
            &self.queue,
            sun,
            self.weather_wind,
            self.pbr_enabled,
        );
        // The CPU solve is already running on the weather worker. Rain and the
        // cloud terrain-interaction experiment share this same heightfield, so
        // either feature may request the GPU upload during the map switch.
        if self.weather.rain.enabled || self.cloud_terrain_interaction {
            self.ensure_weather_occlusion();
        }
        self.rebuild_weather_surface_receiver_bind_group();
        self.update_weather_surface_uniform();
        self.weather.fog.write_legacy_control(&self.queue);
        self.sync_self_legacy_fog();
        upload_timings.post_misc_ms = post_misc_started.elapsed().as_secs_f64() * 1000.0;
        let frame_plan_started = Instant::now();
        self.rebuild_planar_reflection_resources();
        self.rebuild_frame_plan();
        upload_timings.frame_plan_ms = frame_plan_started.elapsed().as_secs_f64() * 1000.0;
        // Loading the world may discover authored/environment planar reflectors,
        // which changes the specialized BSP feature key after build_world had to
        // create its first variant. Activate (or lazily build) the final key now.
        let variant_started = Instant::now();
        self.activate_world_pipeline_variant();
        upload_timings.variant_ms = variant_started.elapsed().as_secs_f64() * 1000.0;
        let finalize_started = Instant::now();
        self.update_lighting_settings();
        upload_timings.finalize_ms = finalize_started.elapsed().as_secs_f64() * 1000.0;
        upload_timings
    }

    /// Entities and procedural grass are not BSP material stages, so they apply
    /// Legacy fog in their own shaders (GodotOcean does the same from its
    /// surface fog in bsp.wgsl). Runs whenever fog settings or map fog change,
    /// alongside the legacy fog control upload; nothing is checked per frame.
    pub(in crate::renderer) fn sync_self_legacy_fog(&mut self) {
        let fog = self.weather.fog.legacy_self_fog().map_or_else(
            EntityLegacyFog::default,
            |(color_depth, params)| EntityLegacyFog {
                color_depth,
                params,
            },
        );
        let scene_format = self.scene_format();
        let samples = self.msaa_samples;
        self.dynamic_model_renderer.set_legacy_fog(
            &mut self.pipeline_jobs,
            &self.device,
            &self.queue,
            scene_format,
            samples,
            fog,
        );
        let grass_local_fog_enabled = self.weather.fog.legacy_effective()
            && self
                .world
                .as_ref()
                .and_then(|world| world.grass.as_ref())
                .is_some_and(GrassMapGpu::has_local_fog);
        let mut grass_fog_params = fog.params;
        grass_fog_params[2] = self.weather.fog.legacy_local_fog_scale();
        self.grass_renderer.set_legacy_fog(
            &self.device,
            &self.queue,
            scene_format,
            samples,
            fog.color_depth,
            grass_fog_params,
            grass_local_fog_enabled,
        );
    }

    /// Advance every `videoMap` cinematic to the wall clock and upload its newest frame.
    pub(in crate::renderer) fn update_world_videos(&mut self) {
        let Some(world) = self.world.as_ref() else {
            return;
        };
        let now = self.started.elapsed().as_secs_f64();
        for player in &mut self.world_videos {
            if now < player.next_frame_at {
                continue;
            }
            let period = 1.0 / f64::from(player.video.fps().max(1));
            let mut decoded = false;
            let mut steps = 0;
            while player.next_frame_at <= now && steps < WORLD_VIDEO_MAX_CATCH_UP {
                if player.video.next_frame().is_none() {
                    player.video.rewind();
                    if player.video.next_frame().is_none() {
                        break;
                    }
                }
                decoded = true;
                player.next_frame_at += period;
                steps += 1;
            }
            if player.next_frame_at <= now {
                player.next_frame_at = now + period;
            }
            let (Some(image), true) = (world.textures.get(player.texture), decoded) else {
                continue;
            };
            let (width, height) = (player.video.width(), player.video.height());
            // The decoder keeps its newest frame until the next call, so the
            // upload source is the frame decoded last above.
            let frame = player.video.current_frame();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &image._texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                frame,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    pub(in crate::renderer) fn unload_map(&mut self) {
        // BLAS/TLAS resources refer to the current world's static geometry.
        self.ray_traced_shadows = None;
        // Returning to the front end is a real world teardown, not just another
        // overlay over the last gameplay frame. Drop all map-owned GPU state so
        // the renderer returns to the same world-less state used at startup.
        self.world = None;
        self.world_videos.clear();
        self.sync_baked_brightness_assets();
        self.inspector_vertex_range = None;
        self.inspector_entity_num = None;
        self.surface_deformation.clear(&self.queue);
        self.weather.rain.start_map_occlusion(None);
        self.weather
            .fog
            .install_map(None, std::iter::empty::<([f32; 4], u64)>());
        self.weather.fog.write_legacy_control(&self.queue);
        self.sync_self_legacy_fog();

        self.history_valid = false;
        self.camera_history_valid = false;
        self.dof_focus_valid = false;
        self.previous_frame_time = 0.0;
        self.previous_unjittered_view_proj = Mat4::IDENTITY;
        self.previous_cloud_view_proj = Mat4::IDENTITY;
        self.cloud_history_valid = false;
        self.cloud_history_read_index = 0;
        self.cloud_temporal_frame_index = 0;
        self.taa_frame_index = 0;
        self.history_read_index = 0;
        self.ssao_history_valid = false;
        self.ssao_history_read_index = 0;
        self.ssr_history_valid = false;
        self.ssr_history_read_index = 0;
        self.ssr_frame_index = 0;

        self.rebuild_planar_reflection_resources();
        self.rebuild_weather_surface_receiver_bind_group();
        self.update_weather_surface_uniform();
        self.rebuild_frame_plan();
    }
}
