//! Frame fast.
use crate::renderer::{
    wireframe_cull_slot,
    world_stage_probe_excludes, aabb_intersects_clip_frustum, active_batch_selection, batch_area_visible, camera_depth_clear,
    draw_snow_shell_fast, draw_world_batch, effective_area_mask, hidden_bsp_model_surface, hidden_inline_batch, inspector_index_range,
    ocean_suppresses_batch, refresh_auto4_lazy_collapse, sample_view_rotation,
    snow_shell_draw_near_center, ui, update_visibility, Camera, CameraUniform, CompanionSceneView,
    CullDebugMode, DrawClass, DynamicModelSurface, FootprintMode, FrameInfo, GpuPass, Instant,
    LatestViewState, PvsMode, RenderError, Renderer, SpectatorHighlight, Vec3, ViewLatchMode,
};

impl Renderer {
    pub(in crate::renderer) fn render_fast_baseline(
        &mut self,
        camera: &Camera,
        player_position: Option<Vec3>,
        area_mask: Option<&[u8; 32]>,
        dynamic_models: &[DynamicModelSurface],
        spectator_highlights: &[SpectatorHighlight],
        companion_scene: Option<&CompanionSceneView>,
        dynamic_crosshair_world: Option<[f32; 3]>,
        view_latch: ViewLatchMode,
        latest_view: Option<&LatestViewState>,
    ) -> Result<FrameInfo, RenderError> {
        let frame_started = Instant::now();
        let needs_early_latch = self.world.as_ref().is_some_and(|world| {
            (self.grass_enabled && world.grass.is_some())
                || !world.surface_sprite_effects.is_empty()
        });
        let mut render_camera = *camera;
        let mut late_view_sample = if needs_early_latch {
            latest_view
                .and_then(|state| sample_view_rotation(&mut render_camera, state, view_latch))
        } else {
            None
        };
        let mut view_proj = render_camera.view_projection(self.config.width, self.config.height);
        let mut uniform = CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos_time: [
                render_camera.position.x,
                render_camera.position.y,
                render_camera.position.z,
                self.started.elapsed().as_secs_f32(),
            ],
            clip_plane: [0.0; 4],
            render_flags: [
                0,
                (u32::from(self.static_bsp_ao_enabled) | (u32::from(self.sky_fractal_override) << 1)
                    | self.ocean_sky_horizon_flag()),
                self.classic_world_render_flags(),
                if self.taa_enabled {
                    self.taa_frame_index
                } else {
                    0
                },
            ],
            camera_forward: render_camera.forward().extend(0.0).to_array(),
            unjittered_view_proj: view_proj.to_cols_array_2d(),
            previous_unjittered_view_proj: view_proj.to_cols_array_2d(),
            jump_shade: [0.0; 4],
        };
        let defer_camera_write = latest_view.is_some() && !needs_early_latch;
        if !defer_camera_write {
            self.queue
                .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));
        }
        self.ensure_wireframe_pipelines();
        self.ensure_shader_complexity_target();
        self.ensure_shader_complexity_pipelines();
        self.update_sun_ray_preview(&render_camera, player_position);
        if self.weather.wind_visuals.active() {
            // Wind visuals are deliberately compatible with FastBaseline: they
            // need only the shared camera/depth target and weather heightfield.
            self.prepare_wind_visuals_frame(uniform.camera_pos_time[3]);
        }

        // One relaxed check per frame; the disabled path does no per-call work.
        let capture_api_calls = crate::renderer::diagnostics::perf_capture::enabled();
        let mut info = FrameInfo {
            world_path: "fast-baseline",
            late_latch_after_acquire: latest_view.is_some() && !needs_early_latch,
            ..FrameInfo::default()
        };
        info.world_shader_programs.requested = self.fragment_shader_mode;
        let snow_deform_center = self.surface_deformation.field_center();
        let mut prepared_grass = None;
        let mut prepared_surface_sprite_effects = None;
        if let Some(world) = &mut self.world {
            // Shell fragments are discarded unless footprints are in 3D mode, so
            // outside it there is nothing to build, hold or draw.
            if self.surface_deformation.mode() == FootprintMode::ThreeD {
                world.snow_shell.stream(&self.device, snow_deform_center);
            } else {
                world.snow_shell.release();
            }
            if needs_early_latch {
                update_visibility(world, &render_camera);
                let auto4_area_mask = effective_area_mask(world, self.pvs_mode, area_mask).copied();
                refresh_auto4_lazy_collapse(
                    &self.queue,
                    world,
                    self.pvs_mode,
                    auto4_area_mask,
                    self.cull_debug_mode != CullDebugMode::Off,
                );
            }
            if self.grass_enabled {
                let pvs_cluster = world.last_cluster.flatten();
                if let Some(grass) = world.grass.as_ref() {
                    prepared_grass = Some(self.grass_renderer.prepare_draw(
                        &mut self.pipeline_jobs,
                        &self.device,
                        &self.queue,
                        grass,
                        render_camera.position,
                        player_position,
                        view_proj,
                        pvs_cluster,
                        uniform.camera_pos_time[3],
                        self.grass_precompute_enabled,
                        self.grass_mid_lod_enabled,
                        self.grass_front_to_back_enabled,
                    ));
                }
            }
            if !world.surface_sprite_effects.is_empty() {
                let pvs_cluster = if matches!(self.pvs_mode, PvsMode::Off) {
                    None
                } else {
                    world.last_cluster.flatten()
                };
                prepared_surface_sprite_effects =
                    Some(self.surface_sprite_effect_renderer.prepare_draw(
                        &mut self.pipeline_jobs,
                        &self.device,
                        &self.queue,
                        &world.surface_sprite_effects,
                        world.surface_sprite_resident.as_ref(),
                        &render_camera,
                        view_proj,
                        pvs_cluster,
                        uniform.camera_pos_time[3] * 1000.0,
                        self.grass_enabled && world.grass.is_some(),
                        self.weather_wind,
                    ));
                info.surface_sprites = prepared_surface_sprite_effects
                    .as_ref().map(|prepared| prepared.stats).unwrap_or_default();
            }
        }
        let dynamic_prepare_started = Instant::now();
        self.dynamic_model_renderer.entity_sun_relight = self.entity_sun_relight();
        self.sync_entity_cloud_shadow_sun();
        self.sync_entity_dlights(false);
        let entity_light_grid = self
            .world
            .as_ref()
            .and_then(|world| world.entity_light_grid.as_ref());
        let split_dynamic_wireframe = self.wireframe_requires_dynamic_class_split();
        self.dynamic_model_renderer.prepare(
            &mut self.baked_brightness,
            &mut self.pipeline_jobs,
            &self.device,
            &self.queue,
            dynamic_models,
            entity_light_grid,
            split_dynamic_wireframe,
            false,
            false,
            false,
        );
        let glass_capture_needed = self.world.as_ref().is_some_and(|world| {
            world.has_glass_primary
        }) || self.dynamic_model_renderer.draws.iter().any(|draw| draw.alpha_mode == crate::renderer::DynamicModelAlphaMode::Glass)
            || self.dynamic_model_renderer.skin_draws.iter().any(|draw| draw.alpha_mode == crate::renderer::DynamicModelAlphaMode::Glass);
        self.ensure_prepared_wireframe_pipelines(
            prepared_grass.as_ref(),
            prepared_surface_sprite_effects.as_ref(),
        );
        self.dynamic_model_renderer.prepare_model_bundles(&self.device, &self.camera_bind_group);
        info.dynamic_model_prepare_ms = dynamic_prepare_started.elapsed().as_secs_f64() * 1000.0;
        let (ghoul2_gpu_instances, ghoul2_gpu_draw_calls) =
            self.dynamic_model_renderer.ghoul2_draw_stats();
        info.ghoul2_gpu_instances = ghoul2_gpu_instances;
        info.ghoul2_gpu_draw_calls = ghoul2_gpu_draw_calls;
        info.dynamic_model_surfaces = u32::try_from(dynamic_models.len()).unwrap_or(u32::MAX);
        info.dynamic_model_vertices = dynamic_models
            .iter()
            .map(|surface| surface.vertex_count() as u64)
            .sum();
        info.dynamic_model_indices = dynamic_models
            .iter()
            .map(|surface| surface.index_count() as u64)
            .sum();
        info.cpu_prepare_ms = frame_started.elapsed().as_secs_f64() * 1000.0;

        let acquire_started = Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.flush_staged_queue_writes();
                return Err(RenderError::Timeout);
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.flush_staged_queue_writes();
                return Err(RenderError::Occluded);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.flush_staged_queue_writes();
                return Err(RenderError::Outdated);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.flush_staged_queue_writes();
                return Err(RenderError::Lost);
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(RenderError::Validation),
        };
        info.cpu_acquire_ms = acquire_started.elapsed().as_secs_f64() * 1000.0;
        let surface_view = self.surface_texture_view(&frame);
        let frame_view = self.hdr_output.as_ref().map_or_else(|| surface_view.clone(), |output| output.view.clone());
        let mut screenshot_readback = self.prepare_screenshot_readback();

        // Last coherent point on the baseline path: everything above is either
        // camera-independent or (when grass/surface sprites are active) forced
        // the earlier latch. CPU frustum tests below consume `view_proj`, so
        // sampling after this point would make culling disagree with the GPU.
        if !needs_early_latch {
            if let Some(state) = latest_view {
                late_view_sample = sample_view_rotation(&mut render_camera, state, view_latch);
                view_proj = render_camera.view_projection(self.config.width, self.config.height);
                uniform.view_proj = view_proj.to_cols_array_2d();
                uniform.camera_pos_time[0] = render_camera.position.x;
                uniform.camera_pos_time[1] = render_camera.position.y;
                uniform.camera_pos_time[2] = render_camera.position.z;
                uniform.camera_forward = render_camera.forward().extend(0.0).to_array();
                uniform.unjittered_view_proj = view_proj.to_cols_array_2d();
                uniform.previous_unjittered_view_proj = view_proj.to_cols_array_2d();
            }
            if let Some(world) = &mut self.world {
                update_visibility(world, &render_camera);
                // The cluster may have changed: AUTO 4 must select that
                // cluster's plan before the draw loop reads it.
                let auto4_area_mask = effective_area_mask(world, self.pvs_mode, area_mask).copied();
                refresh_auto4_lazy_collapse(
                    &self.queue,
                    world,
                    self.pvs_mode,
                    auto4_area_mask,
                    self.cull_debug_mode != CullDebugMode::Off,
                );
            }
        }
        if defer_camera_write {
            self.queue
                .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));
        }
        info.late_latch = late_view_sample;
        if glass_capture_needed {
            let key = (self.config.width.max(1), self.config.height.max(1), self.msaa_samples, self.scene_format());
            if self.ocean_optics.key != key {
                self.ocean_optics = crate::ocean::optics::Resources::new(
                    &self.device, &self.ocean_optics_layout, &self.ocean_layout, key,
                );
            }
            let optics = crate::ocean::optics::Uniform {
                inverse_view_projection: view_proj.inverse().to_cols_array_2d(),
                eye: render_camera.position.extend(1.0).to_array(),
                forward: render_camera.forward().extend(0.0).to_array(),
                fog: [0.0; 4], params: [0.0; 4],
                minimum: [[0.0; 4]; 8], maximum: [[0.0; 4]; 8],
                sky_regions: [crate::ocean::optics::SkyFogRegion::default(); 8],
                sky_info: [0.0; 4],
                caustic_tuning: [1.0, 1.0, 0.0, 0.0],
                procedural_params: [0.0; 4],
                eye_wave: [0.0; 4],
            };
            self.queue.write_buffer(&self.ocean_optics.uniform, 0, bytemuck::bytes_of(&optics));
        }
        if glass_capture_needed {
            self.prepare_glass();
        }
        self.dynamic_model_renderer.glass_bind_group =
            glass_capture_needed.then(|| self.ocean_optics.bind_group.clone());
        let crosshair_world = late_view_sample
            .map(|measurement| measurement.crosshair_world)
            .unwrap_or(dynamic_crosshair_world);
        self.rebuild_player_name_tail(view_proj, crosshair_world);

        let perf_timing = crate::renderer::diagnostics::perf_capture::enabled();
        let encode_started = Instant::now();
        // Profiler hooks are no-ops unless r_gpuTimings is on; they let the
        // baseline's world/post passes be timed directly against the unified path.
        self.gpu_profiler.begin_frame(&self.device);
        let perf_setup_started = perf_timing.then(Instant::now);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA fast baseline frame encoder"),
            });
        if let Some(started) = perf_setup_started {
            info.perf_encode_setup_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        let perf_pre_world_started = perf_timing.then(Instant::now);
        self.gpu_profiler
            .write_encoder_timestamp(&mut encoder, GpuPass::Frame, false);
        self.encode_inline_transforms(&mut encoder);
        // A transmissive snapshot needs a sampleable scene attachment. The
        // swapchain view is renderable but is not guaranteed TEXTURE_BINDING.
        let use_gamma_post = self.frame_plan.use_post || glass_capture_needed;
        let menu_backdrop_active = self.menu_backdrop_requested() && self.menu_backdrop.is_some();
        let crt_active = self.crt_scope != ui::CrtScope::Off;
        if crt_active {
            self.ensure_crt_resources();
        }
        let crt_view = self.crt_resources.as_ref().map(|resources| resources.view.clone());
        let crt_everything = self.crt_scope == ui::CrtScope::Everything;
        let crt_before_ui = crt_active && !crt_everything;
        let direct_presentation_output = if menu_backdrop_active {
            &self
                .menu_backdrop
                .as_ref()
                .expect("menu backdrop resources must exist while active")
                .view
        } else {
            &frame_view
        };
        let crt_scene_target = crt_active && !(crt_everything && menu_backdrop_active);
        let presentation_output = if crt_scene_target {
            crt_view.as_ref().expect("CRT resources must exist while enabled")
        } else {
            direct_presentation_output
        };
        let scene_output = if use_gamma_post {
            &self.targets.scene_view
        } else {
            presentation_output
        };

        if self.grass_enabled {
            if let (Some(world), Some(prepared)) = (&self.world, prepared_grass.as_ref()) {
                if let Some(grass) = world.grass.as_ref() {
                    self.grass_renderer.encode_prepare(
                        &mut encoder,
                        grass,
                        prepared,
                        &self.shadow_resources.receiver_bind_group,
                    );
                }
            }
        }

        if let Some(started) = perf_pre_world_started {
            info.perf_encode_pre_world_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        let perf_scene_started = perf_timing.then(Instant::now);
        if let Some(world) = &self.world {
            let (color_view, resolve_target) = if let Some(msaa_view) = &self.targets.msaa_view {
                (msaa_view, Some(scene_output))
            } else {
                (scene_output, None)
            };

            let active = active_batch_selection(world, self.pvs_mode);
            let glass_capture_needed = glass_capture_needed && (
                active.indices.iter().any(|&index| active.batches[index].source.glass_primary)
                    || self.dynamic_model_renderer.draws.iter().any(|draw| draw.alpha_mode == crate::renderer::DynamicModelAlphaMode::Glass)
                    || self.dynamic_model_renderer.skin_draws.iter().any(|draw| draw.alpha_mode == crate::renderer::DynamicModelAlphaMode::Glass)
            );
            let effective_area_mask = effective_area_mask(world, self.pvs_mode, area_mask);
            info.world_pvs_batches = u32::try_from(active.indices.len()).unwrap_or(u32::MAX);
            if capture_api_calls {
                for mover in &world.inline_models {
                    match mover.uploaded {
                        Some(Some(_)) => info.perf_inline_models_active += 1,
                        Some(None) => info.perf_inline_models_hidden += 1,
                        None => info.perf_inline_models_uninitialized += 1,
                    }
                }
            }

            self.gpu_profiler
                .write_encoder_timestamp(&mut encoder, GpuPass::World, false);
            if capture_api_calls { info.perf_encode_calls.render_passes = info.perf_encode_calls.render_passes.saturating_add(1); }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA world (fast baseline)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.weather.fog.clear_color()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(camera_depth_clear()),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
            pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
            if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
            if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
            pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
            if capture_api_calls { info.perf_encode_calls.set_index_buffer = info.perf_encode_calls.set_index_buffer.saturating_add(1); }
            pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            let mut current_pipeline = None;
            let mut grass_drawn = false;
            let mut snow_shell_drawn = false;
            // Reuse retained storage: transparent BSP stages must run after
            // depth-writing dynamic models even in the fast baseline.
            let mut deferred_transparent =
                std::mem::take(&mut self.post_ocean_transparent_scratch);
            deferred_transparent.clear();
            let mut transparency_order = super::plan::WorldTransparencyOrder::default();
            for (active_pos, &batch_index) in active.indices.iter().enumerate() {
                let batch = &active.batches[batch_index];
                if hidden_bsp_model_surface(self.draw_bsp_model_surfaces, batch, &world.material_debug) {
                    continue;
                }
                if world_stage_probe_excludes(self.world_stage_probe, batch.source.pipeline, batch.source.water) {
                    continue;
                }
                if !self.draw_inline_models && batch.is_inline_entity {
                    continue;
                }
                if hidden_inline_batch(batch, &world.inline_models) {
                    if capture_api_calls {
                        info.perf_count_hidden_world_stage(
                            batch.source.material_debug_index,
                            batch.indexed_range.end.saturating_sub(batch.indexed_range.start),
                        );
                    }
                    continue;
                }
                if !batch_area_visible(batch, effective_area_mask) {
                    continue;
                }
                if ocean_suppresses_batch(self.ocean_enabled, batch) {
                    continue;
                }
                // OpenJK does not stop at PVS: R_RecursiveWorldNode frustum-culls
                // BSP node bounds before adding leaf surfaces. Compiled BSP worlds
                // need the same second-stage rejection. The old source_map guard
                // accidentally disabled it for normal .bsp maps.
                if batch.source.pipeline.class != DrawClass::Sky
                    && !aabb_intersects_clip_frustum(batch.bounds_min, batch.bounds_max, view_proj)
                {
                    info.world_frustum_rejected = info.world_frustum_rejected.saturating_add(1);
                    continue;
                }
                info.world_encoded_batches = info.world_encoded_batches.saturating_add(1);
                if !grass_drawn && batch.source.pipeline.class == DrawClass::Transparent {
                    if !snow_shell_drawn {
                        draw_snow_shell_fast(
                            &mut pass,
                            world,
                            view_proj,
                            snow_deform_center,
                            self.draw_bsp_model_surfaces,
                            &mut current_pipeline,
                        );
                        if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                        pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                        snow_shell_drawn = true;
                    }
                    if self.grass_enabled {
                        if let (Some(grass), Some(prepared)) =
                            (&world.grass, prepared_grass.as_ref())
                        {
                            self.gpu_profiler.write_render_pass_timestamp(
                                &mut pass,
                                GpuPass::Grass,
                                false,
                            );
                            info.grass = self.grass_renderer.draw(
                                &mut pass,
                                &self.camera_bind_group,
                                &self.shadow_resources.receiver_bind_group,
                                grass,
                                prepared,
                            );
                            self.gpu_profiler.write_render_pass_timestamp(
                                &mut pass,
                                GpuPass::Grass,
                                true,
                            );
                            if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                            if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                            pass.set_vertex_buffer(
                                1,
                                world.legacy_dlight_surface_id_buffer.slice(..),
                            );
                            if capture_api_calls { info.perf_encode_calls.set_index_buffer = info.perf_encode_calls.set_index_buffer.saturating_add(1); }
                            pass.set_index_buffer(
                                world.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                            pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                            current_pipeline = None;
                        }
                    }
                    grass_drawn = true;
                }
                let capture_decal = super::plan::WorldTransparencyOrder::capture_decal(
                    glass_capture_needed, batch.source.pipeline.offset, batch.source.late_sorted_material,
                );
                if batch.source.glass_primary || (!capture_decal && transparency_order.defer_stage(
                    &batch.source.vertices,
                    batch.source.bsp_shader_index,
                    batch.source.pipeline.class,
                    batch.source.opaque_composite_before_clouds,
                    batch.source.late_sorted_material,
                    batch.source.authored_sort_priority < crate::scene::SS_OPAQUE_SORT_PRIORITY,
                )) {
                    deferred_transparent.push(active_pos);
                    continue;
                }
                if current_pipeline != Some(batch.source.pipeline) {
                    if capture_api_calls { info.perf_encode_calls.set_pipeline = info.perf_encode_calls.set_pipeline.saturating_add(1); }
                    pass.set_pipeline(&world.fast_pipelines[&batch.source.pipeline]);
                    current_pipeline = Some(batch.source.pipeline);
                }
                if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                pass.set_bind_group(1, &batch.fast_bind_group, &[]);
                if capture_api_calls { info.perf_encode_calls.world_helper_draw_indexed = info.perf_encode_calls.world_helper_draw_indexed.saturating_add(1); }
                draw_world_batch(&mut pass, batch, 0..1, self.ocean_clipmap_quality());
            }
            if !snow_shell_drawn {
                draw_snow_shell_fast(
                    &mut pass,
                    world,
                    view_proj,
                    snow_deform_center,
                    self.draw_bsp_model_surfaces,
                    &mut current_pipeline,
                );
                if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
            }
            if !grass_drawn && self.grass_enabled {
                if let (Some(grass), Some(prepared)) = (&world.grass, prepared_grass.as_ref()) {
                    self.gpu_profiler
                        .write_render_pass_timestamp(&mut pass, GpuPass::Grass, false);
                    info.grass = self.grass_renderer.draw(
                        &mut pass,
                        &self.camera_bind_group,
                        &self.shadow_resources.receiver_bind_group,
                        grass,
                        prepared,
                    );
                    self.gpu_profiler
                        .write_render_pass_timestamp(&mut pass, GpuPass::Grass, true);
                    if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                    pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                    if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                    pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                    if capture_api_calls { info.perf_encode_calls.set_index_buffer = info.perf_encode_calls.set_index_buffer.saturating_add(1); }
                    pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                    pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                }
            }
            if let Some(prepared) = prepared_surface_sprite_effects.as_ref() {
                self.gpu_profiler.write_render_pass_timestamp(&mut pass, GpuPass::SurfaceSprites, false);
                self.surface_sprite_effect_renderer.draw(
                    &mut pass,
                    &self.camera_bind_group,
                    &world.surface_sprite_effects,
                    world.surface_sprite_resident.as_ref(),
                    prepared,
                );
                self.gpu_profiler.write_render_pass_timestamp(&mut pass, GpuPass::SurfaceSprites, true);
            }
            if let Some(ocean) = self.ocean.as_ref() {
                ocean.draw_spray(
                    &mut pass,
                    &self.camera_bind_group,
                    self.ocean_spray_pipeline.as_ref(),
                );
            }
            for (_, ocean) in &self.authored_oceans {
                ocean.draw_spray(
                    &mut pass,
                    &self.camera_bind_group,
                    self.ocean_spray_pipeline.as_ref(),
                );
            }
            if glass_capture_needed && self.glass_world.as_ref().is_some_and(|glass| glass.settings.needs_background()) {
                drop(pass);
                self.ocean_optics.capture_background(&self.device, &mut encoder, color_view,
                    &self.targets.depth_view, &self.ocean_inert_bind_group);
                pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("JKA fast models after glass background capture"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: color_view, depth_slice: None, resolve_target,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.targets.depth_view,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                        stencil_ops: None,
                    }), ..Default::default()
                });
            }
            self.gpu_profiler
                .write_render_pass_timestamp(&mut pass, GpuPass::DynamicModels, false);
            self.dynamic_model_renderer.draw_phase(
                &mut pass, &self.camera_bind_group, true, None, None, None, false,
            );
            self.gpu_profiler
                .write_render_pass_timestamp(&mut pass, GpuPass::DynamicModels, true);
            if glass_capture_needed {
                drop(pass);
                self.ocean_optics.capture(
                    &self.device, &mut encoder, color_view, &self.targets.depth_view,
                    &self.ocean_inert_bind_group, None,
                );
                self.glass_world.as_ref().expect("glass pipeline").capture_geometry(
                    &mut encoder, &self.ocean_optics, &self.camera_bind_group, world,
                    &active.batches, &active.indices, &deferred_transparent,
                    &self.dynamic_model_renderer, false,
                );
                pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("JKA fast glass transmission"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: color_view, depth_slice: None, resolve_target,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.targets.depth_view,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
            }
            if !deferred_transparent.is_empty() {
                // The model renderer changed the vertex/index streams and bind
                // groups. Restore the fast world state once, then draw the
                // originally visible transparent batches in authored order.
                if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                if capture_api_calls { info.perf_encode_calls.set_index_buffer = info.perf_encode_calls.set_index_buffer.saturating_add(1); }
                pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                current_pipeline = None;
                for &active_pos in &deferred_transparent {
                    let batch = &active.batches[active.indices[active_pos]];
                    if batch.source.glass_primary {
                        pass.set_pipeline(&self.glass_world.as_ref().expect("glass pipeline").pipelines[&batch.source.pipeline.cull]);
                        pass.set_bind_group(0, &self.camera_bind_group, &[]);
                        pass.set_bind_group(1, &batch.bind_group, &[]);
                        pass.set_bind_group(2, &self.ocean_optics.bind_group, &[]);
                        draw_world_batch(&mut pass, batch, 0..1, None);
                        pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                        current_pipeline = None;
                        continue;
                    }
                    if current_pipeline != Some(batch.source.pipeline) {
                        if capture_api_calls { info.perf_encode_calls.set_pipeline = info.perf_encode_calls.set_pipeline.saturating_add(1); }
                        pass.set_pipeline(&world.fast_pipelines[&batch.source.pipeline]);
                        current_pipeline = Some(batch.source.pipeline);
                    }
                    if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                    pass.set_bind_group(1, &batch.fast_bind_group, &[]);
                    if capture_api_calls { info.perf_encode_calls.world_helper_draw_indexed = info.perf_encode_calls.world_helper_draw_indexed.saturating_add(1); }
                    draw_world_batch(&mut pass, batch, 0..1, self.ocean_clipmap_quality());
                }
            }
            self.post_ocean_transparent_scratch = deferred_transparent;
            // Alpha-blended entities/FX remain after world transparency.
            self.gpu_profiler.write_render_pass_timestamp(
                &mut pass, GpuPass::DynamicModelsTranslucent, false,
            );
            self.dynamic_model_renderer.draw_phase(
                &mut pass, &self.camera_bind_group, false, None, None, None, false,
            );
            self.gpu_profiler.write_render_pass_timestamp(
                &mut pass, GpuPass::DynamicModelsTranslucent, true,
            );
            self.weather
                .wind_visuals
                .render(&mut pass, &self.camera_bind_group);

            // DynamicModelRenderer::draw() binds its own transient entity vertex/index
            // buffers. Everything below this point (wireframe, surface inspector, PVS
            // debug) addresses BSP/world index ranges, so restore the world geometry
            // bindings unconditionally before any of those debug draws run.
            if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
            if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
            pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
            if capture_api_calls { info.perf_encode_calls.set_index_buffer = info.perf_encode_calls.set_index_buffer.saturating_add(1); }
            pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

            if self.wireframe_mask != 0 {
                let mask = self.wireframe_mask;
                if mask & (ui::wireframe::MAP | ui::wireframe::ENTITIES) != 0 {
                    if self.wireframe_pipeline.iter().all(Option::is_some) {
                        let mut bound_cull = None;
                        if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                        pass.set_bind_group(0, &self.camera_bind_group, &[]);
                        for &batch_index in active.indices {
                            let batch = &active.batches[batch_index];
                            let category_enabled = if batch.is_inline_entity {
                                mask & ui::wireframe::ENTITIES != 0
                            } else {
                                mask & ui::wireframe::MAP != 0
                            };
                            if category_enabled
                                // World wireframe must follow the resolved remap, not
                                // resurrect framebuffer/depth no-op shaders (e.g. clear).
                                && batch.source.contributes_to_world_wireframe()
                                && !hidden_bsp_model_surface(self.draw_bsp_model_surfaces, batch, &world.material_debug)
                                && (!batch.is_inline_entity || self.draw_inline_models)
                                && !hidden_inline_batch(batch, &world.inline_models)
                                && (!self.ocean_enabled || batch.ocean_clipmap.is_none())
                                && batch.source.pipeline.class != DrawClass::Sky
                                && aabb_intersects_clip_frustum(
                                    batch.bounds_min,
                                    batch.bounds_max,
                                    view_proj,
                                )
                            {
                                let slot = wireframe_cull_slot(batch.source.pipeline.cull);
                                if bound_cull != Some(slot) {
                                    if let Some(pipeline) = &self.wireframe_pipeline[slot] {
                                        if capture_api_calls { info.perf_encode_calls.set_pipeline = info.perf_encode_calls.set_pipeline.saturating_add(1); }
                                        pass.set_pipeline(pipeline);
                                    }
                                    bound_cull = Some(slot);
                                }
                                if capture_api_calls { info.perf_encode_calls.world_helper_draw_indexed = info.perf_encode_calls.world_helper_draw_indexed.saturating_add(1); }
                                draw_world_batch(&mut pass, batch, 0..1, None);
                            }
                        }
                    }
                }

                if mask & ui::wireframe::DEFORMATION != 0 {
                    if let Some(pipeline) = &self.fast_world_wireframe_pipeline {
                        if capture_api_calls { info.perf_encode_calls.set_pipeline = info.perf_encode_calls.set_pipeline.saturating_add(1); }
                        pass.set_pipeline(pipeline);
                        if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                        pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                        if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                        pass.set_vertex_buffer(
                            1,
                            world.snow_shell.legacy_dlight_surface_id_buffer.slice(..),
                        );
                        for chunk in world.snow_shell.chunks.values() {
                            let Some(vertex_buffer) = &chunk.vertex_buffer else {
                                continue;
                            };
                            if !chunk.near_center(snow_deform_center) {
                                continue;
                            }
                            if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                            for draw in &chunk.draws {
                                if !snow_shell_draw_near_center(draw, snow_deform_center)
                                    || !aabb_intersects_clip_frustum(
                                        draw.bounds_min,
                                        draw.bounds_max,
                                        view_proj,
                                    )
                                {
                                    continue;
                                }
                                let Some(batch) = world.coarse_batches.get(draw.coarse_batch_index)
                                else {
                                    continue;
                                };
                                if hidden_bsp_model_surface(self.draw_bsp_model_surfaces, batch, &world.material_debug) {
                                    continue;
                                }
                                if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                                pass.set_bind_group(1, &batch.fast_bind_group, &[]);
                                if capture_api_calls { info.perf_encode_calls.draw = info.perf_encode_calls.draw.saturating_add(1); }
                                pass.draw(draw.vertices.clone(), 1..2);
                            }
                        }
                    }
                }

                if mask & ui::wireframe::GRASS != 0 {
                    if let (Some(grass), Some(prepared)) = (&world.grass, prepared_grass.as_ref()) {
                        self.grass_renderer.draw_wireframe(
                            &mut pass,
                            &self.camera_bind_group,
                            &self.shadow_resources.receiver_bind_group,
                            grass,
                            prepared,
                        );
                    }
                }

                if mask & ui::wireframe::EFFECTS != 0 {
                    if let Some(prepared) = prepared_surface_sprite_effects.as_ref() {
                        self.surface_sprite_effect_renderer.draw_wireframe(
                            &mut pass,
                            &self.camera_bind_group,
                            &world.surface_sprite_effects,
                            world.surface_sprite_resident.as_ref(),
                            prepared,
                        );
                    }
                }

                let dynamic_mask =
                    ui::wireframe::PLAYERS | ui::wireframe::ENTITIES | ui::wireframe::EFFECTS;
                if mask & dynamic_mask != 0 {
                    self.dynamic_model_renderer.draw_wireframe(
                        &mut pass,
                        &self.camera_bind_group,
                        mask,
                    );
                }

                // Grass/dynamic/deformation overlays all bind their own geometry.
                // Diagnostics below still address BSP index ranges.
                if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                if capture_api_calls { info.perf_encode_calls.set_index_buffer = info.perf_encode_calls.set_index_buffer.saturating_add(1); }
                pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            }
            if self.debug_volumes.draw(&mut pass, &self.camera_bind_group) {
                // The overlay bound its own buffers; the inspector below addresses BSP ranges.
                if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                if capture_api_calls { info.perf_encode_calls.set_vertex_buffer = info.perf_encode_calls.set_vertex_buffer.saturating_add(1); }
                pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                if capture_api_calls { info.perf_encode_calls.set_index_buffer = info.perf_encode_calls.set_index_buffer.saturating_add(1); }
                pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            }
            if let (Some(vertices), Some(pipeline)) = (
                &self.inspector_vertex_range,
                self.surface_inspector_pipeline.as_ref(),
            ) {
                if let Some(indices) = inspector_index_range(active.batches, vertices) {
                    if capture_api_calls { info.perf_encode_calls.set_pipeline = info.perf_encode_calls.set_pipeline.saturating_add(1); }
                    pass.set_pipeline(pipeline);
                    if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                    pass.set_bind_group(0, &self.camera_bind_group, &[]);
                    if capture_api_calls { info.perf_encode_calls.draw_indexed = info.perf_encode_calls.draw_indexed.saturating_add(1); }
                    pass.draw_indexed(indices, 0, 0..1);
                }
            }
            // The main render pass must end before opening the cost target pass.
            drop(pass);

            // BSP shader complexity: separate float accumulation and compositing.
            // Runs only when enabled and diagnostic pipelines are ready.
            if self.shader_complexity_enabled {
                if let (Some(target), false) = (
                    self.shader_complexity_target.as_ref(),
                    world.fast_complexity_pipelines.is_empty(),
                ) {
                    {
                        let color = target.multisampled_view.as_ref().unwrap_or(&target.resolved_view);
                        let resolve = target.multisampled_view.as_ref().map(|_| &target.resolved_view);
                        let mut cost_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("BSP cost accumulation (fast)"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: color, depth_slice: None, resolve_target: resolve,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                            })],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: &self.targets.depth_view,
                                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                                stencil_ops: None,
                            }),
                            timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                        });
                        cost_pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                        cost_pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                        cost_pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                        cost_pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                        let mut last_key = None;
                        for &batch_index in active.indices {
                            let batch = &active.batches[batch_index];
                            if hidden_bsp_model_surface(self.draw_bsp_model_surfaces, batch, &world.material_debug) { continue; }
                            if ocean_suppresses_batch(self.ocean_enabled, batch)
                                || world_stage_probe_excludes(self.world_stage_probe, batch.source.pipeline, batch.source.water)
                                || batch.source.pipeline.class == DrawClass::Sky
                                || batch.ocean_clipmap.is_some()
                                || (!self.draw_inline_models && batch.is_inline_entity)
                                || hidden_inline_batch(batch, &world.inline_models)
                                || !batch_area_visible(batch, effective_area_mask)
                                || !aabb_intersects_clip_frustum(batch.bounds_min, batch.bounds_max, view_proj)
                            { continue; }
                            if last_key != Some(batch.source.pipeline) {
                                let Some(pipeline) = world.fast_complexity_pipelines.get(&batch.source.pipeline) else { continue; };
                                cost_pass.set_pipeline(pipeline);
                                last_key = Some(batch.source.pipeline);
                            }
                            cost_pass.set_bind_group(1, &batch.fast_bind_group, &[]);
                            draw_world_batch(&mut cost_pass, batch, 0..1, None);
                        }
                    }
                    target.composite(&mut encoder, self.targets.msaa_view.as_ref(), scene_output);
                }
            }
        } else {
            // Missing BSP / worldless playback is still a valid CGame scene.
            // The old fallback only cleared the target, so player/entity Ghoul2
            // surfaces were prepared above and then silently never submitted.
            // Keep an empty depth-bearing scene and draw dynamic presentation.
            let (color_view, resolve_target) = if let Some(msaa_view) = &self.targets.msaa_view {
                (msaa_view, Some(scene_output))
            } else {
                (scene_output, None)
            };
            if capture_api_calls { info.perf_encode_calls.render_passes = info.perf_encode_calls.render_passes.saturating_add(1); }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA worldless dynamic scene (fast baseline)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.weather.fog.clear_color()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(camera_depth_clear()),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.gpu_profiler
                .write_render_pass_timestamp(&mut pass, GpuPass::DynamicModels, false);
            self.dynamic_model_renderer
                .draw(&mut pass, &self.camera_bind_group);
            self.gpu_profiler
                .write_render_pass_timestamp(&mut pass, GpuPass::DynamicModels, true);
            self.weather
                .wind_visuals
                .render(&mut pass, &self.camera_bind_group);
            let dynamic_mask =
                ui::wireframe::PLAYERS | ui::wireframe::ENTITIES | ui::wireframe::EFFECTS;
            if self.wireframe_mask & dynamic_mask != 0 {
                self.dynamic_model_renderer.draw_wireframe(
                    &mut pass,
                    &self.camera_bind_group,
                    self.wireframe_mask,
                );
            }
            self.debug_volumes.draw(&mut pass, &self.camera_bind_group);
        }

        self.gpu_profiler
            .write_encoder_timestamp(&mut encoder, GpuPass::World, true);
        if self.world_flares_enabled && self.world.is_some() {
            let (flare_color_view, flare_resolve) = if let Some(view) = &self.targets.msaa_view {
                (view, Some(scene_output))
            } else {
                (scene_output, None)
            };
            let flare_format = if use_gamma_post {
                self.scene_format()
            } else {
                self.config.format
            };
            self.world_flares.draw(
                &self.device,
                &self.queue,
                &mut encoder,
                flare_color_view,
                flare_resolve,
                &self.targets.depth_view,
                flare_format,
                self.msaa_samples,
                &render_camera,
                self.config.width,
                self.config.height,
                self.world.as_ref().and_then(|world| world.visibility.as_ref()),
                self.pvs_mode != PvsMode::Off,
                self.weather.fog.legacy_drawfog_value(),
            );
        }
        if let Some(started) = perf_scene_started {
            info.perf_encode_scene_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        let perf_post_ui_started = perf_timing.then(Instant::now);
        if use_gamma_post {
            self.gpu_profiler
                .write_encoder_timestamp(&mut encoder, GpuPass::Post, false);
            if capture_api_calls { info.perf_encode_calls.render_passes = info.perf_encode_calls.render_passes.saturating_add(1); }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA gamma/color post (fast baseline)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: presentation_output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.frame_plan.lut_post {
                if capture_api_calls { info.perf_encode_calls.set_pipeline = info.perf_encode_calls.set_pipeline.saturating_add(1); }
                pass.set_pipeline(&self.gamma_post_pipeline);
                if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                pass.set_bind_group(0, &self.gamma_post_bind_group, &[]);
            } else {
                // Preserve the original smaller shader/bind group for gamma
                // alone. No grading code runs on the maximum-FPS baseline.
                if capture_api_calls { info.perf_encode_calls.set_pipeline = info.perf_encode_calls.set_pipeline.saturating_add(1); }
                pass.set_pipeline(&self.fast_post_pipeline);
                if capture_api_calls { info.perf_encode_calls.set_bind_group = info.perf_encode_calls.set_bind_group.saturating_add(1); }
                pass.set_bind_group(0, &self.fast_post_bind_group, &[]);
            }
            if capture_api_calls { info.perf_encode_calls.draw = info.perf_encode_calls.draw.saturating_add(1); }
            pass.draw(0..3, 0..1);
            drop(pass);
            self.gpu_profiler
                .write_encoder_timestamp(&mut encoder, GpuPass::Post, true);
        }

        self.spectator_highlight.encode(
            &self.device,
            &mut encoder,
            presentation_output,
            &self.dynamic_model_renderer,
            &self.camera_bind_group,
            &self.targets.depth_view,
            spectator_highlights,
            self.config.width,
            self.config.height,
            self.msaa_samples,
        );

        let native_ui_in_main = !menu_backdrop_active
            && match self.crt_scope {
                ui::CrtScope::Off => !self.egui_active,
                ui::CrtScope::Scene => false,
                ui::CrtScope::SceneHud => true,
                ui::CrtScope::Everything => !self.egui_active,
            };
        if native_ui_in_main
            && (self.screen_fx_vertex_count != 0
                || self.ui_vertex_count != 0
                || self.ui_dynamic_vertex_count != 0
                || self.ui_transient_vertex_count != 0)
        {
            if capture_api_calls { info.perf_encode_calls.render_passes = info.perf_encode_calls.render_passes.saturating_add(1); }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA overlay (fast baseline)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: presentation_output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            Self::draw_screen_fx_batches(
                &mut pass,
                &self.screen_fx_vertex_buffer,
                self.screen_fx_vertex_count,
                &self.screen_fx_batches,
                &self.screen_fx_pipelines,
                &self.screen_fx_textures,
                &self.screen_fx_white_bind_group,
            );
            Self::draw_ui_batches(
                &mut pass,
                &self.ui_pipeline,
                &self.ui_bind_group,
                &self.ui_dynamic_vertex_buffer,
                self.ui_dynamic_vertex_count,
                &self.ui_transient_vertex_buffer,
                self.ui_transient_vertex_count,
                &self.ui_vertex_buffer,
                self.ui_vertex_count,
            );
        }

        if self.hdr_output.is_none() && !menu_backdrop_active && !self.egui_active && !crt_active {
            if let Some(readback) = screenshot_readback.as_mut() {
                let staging_buffer = &self
                    .screenshot_readback_buffer
                    .as_ref()
                    .expect("screenshot staging buffer must exist after preparation")
                    .buffer;
                Self::encode_screenshot_copy(
                    &mut encoder,
                    &frame.texture,
                    staging_buffer,
                    readback,
                );
                readback.inline_copy = true;
            }
        }
        let crt_encoded_in_main = crt_before_ui;
        if crt_encoded_in_main {
            // FastBaseline has no external SMAA resolve, so CRT can stay in the
            // same command buffer and costs no additional queue submit.
            self.encode_crt(&mut encoder, direct_presentation_output);
        }
        let companion_scene_frame = companion_scene
            .and_then(|view| self.encode_companion_scene(&mut encoder, view, area_mask));
        self.gpu_profiler
            .write_encoder_timestamp(&mut encoder, GpuPass::Frame, true);
        self.gpu_profiler.resolve(&mut encoder);
        if let Some(started) = perf_post_ui_started {
            info.perf_encode_post_ui_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        let perf_finish_started = perf_timing.then(Instant::now);
        let command_buffer = encoder.finish();
        if let Some(started) = perf_finish_started {
            info.perf_encoder_finish_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        let perf_readback_started = perf_timing.then(Instant::now);
        let profiler_readback_copy = self.gpu_profiler.encode_readback_copy(&self.device);
        if let Some(started) = perf_readback_started {
            info.perf_profiler_readback_encode_ms = started.elapsed().as_secs_f64() * 1000.0;
            info.perf_primary_command_buffers = 1 + u32::from(profiler_readback_copy.is_some());
        }
        info.cpu_encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;
        let submit_started = Instant::now();
        info.submit_call_at = Some(submit_started);
        let perf_submit_api_started = perf_timing.then(Instant::now);
        let frame_submission = if let Some(readback_copy) = profiler_readback_copy {
            self.queue.submit([command_buffer, readback_copy])
        } else {
            self.queue.submit([command_buffer])
        };
        if let Some(started) = perf_submit_api_started {
            info.perf_primary_submit_api_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        let perf_followup_started = perf_timing.then(Instant::now);
        if let Some(scene_frame) = companion_scene_frame {
            self.companion_scene_ready.push(scene_frame);
        }
        if let Some(readback) = screenshot_readback
            .as_mut()
            .filter(|readback| readback.inline_copy)
        {
            readback.submission_index = Some(frame_submission.clone());
        }
        let ui_composite_target = if crt_everything {
            crt_view.as_ref().expect("CRT resources must exist while enabled")
        } else {
            &frame_view
        };
        if menu_backdrop_active {
            self.submit_menu_backdrop(ui_composite_target);
        }
        self.submit_egui(ui_composite_target);
        let late_native_ui = !native_ui_in_main
            && (menu_backdrop_active || self.egui_active || self.crt_scope == ui::CrtScope::Scene);
        if late_native_ui {
            self.submit_ui_overlay(ui_composite_target);
        }
        if crt_everything {
            self.submit_crt(&frame_view);
        }
        if let Some(output) = &self.hdr_output {
            output.submit(&self.device, &self.queue, &surface_view, screenshot_readback.is_some());
        }
        if let Some(readback) = screenshot_readback
            .as_mut()
            .filter(|readback| !readback.inline_copy)
        {
            self.submit_screenshot_copy(self.hdr_output.as_ref().map_or(&frame.texture, |output| &output.capture), readback);
        }
        if let Some(started) = perf_followup_started {
            info.perf_followup_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        info.cpu_submit_ms = submit_started.elapsed().as_secs_f64() * 1000.0;
        let present_started = Instant::now();
        self.window.pre_present_notify();
        self.queue.present(frame);
        let present_completed_at = Instant::now();
        self.model_frame_log.presented(
            present_completed_at,
            view_proj,
            view_proj,
            render_camera.position,
            [self.config.width, self.config.height],
        );
        info.cpu_present_ms = present_completed_at
            .saturating_duration_since(present_started)
            .as_secs_f64()
            * 1000.0;
        info.present_call_completed_at = Some(present_completed_at);
        if let Some(readback) = screenshot_readback {
            self.finish_screenshot_readback(readback);
        }
        self.gpu_profiler.submit();
        info.gpu_ms = self.gpu_profiler.latest_frame_ms();
        info.frame_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
        Ok(info)
    }
}
