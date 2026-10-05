//! Frame fast.
use crate::renderer::{
    aabb_intersects_clip_frustum, active_batch_selection, batch_area_visible, camera_depth_clear,
    draw_snow_shell_fast, draw_world_batch, effective_area_mask, inspector_index_range,
    ocean_suppresses_batch, refresh_auto4_lazy_collapse, sample_view_rotation,
    snow_shell_draw_near_center, ui, update_visibility, Camera, CameraUniform, CompanionSceneView,
    CullDebugMode, DrawClass, DynamicModelSurface, FootprintMode, FrameInfo, GpuPass, Instant,
    LatestViewState, PvsMode, RenderError, Renderer, Vec3, ViewLatchMode,
};

impl Renderer {
    pub(in crate::renderer) fn render_fast_baseline(
        &mut self,
        camera: &Camera,
        player_position: Option<Vec3>,
        area_mask: Option<&[u8; 32]>,
        dynamic_models: &[DynamicModelSurface],
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
                u32::from(self.static_bsp_ao_enabled),
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
        self.update_sun_ray_preview(&render_camera, player_position);

        let mut info = FrameInfo {
            world_path: "fast-baseline",
            ..FrameInfo::default()
        };
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
                        &render_camera,
                        pvs_cluster,
                        uniform.camera_pos_time[3] * 1000.0,
                    ));
            }
        }
        let dynamic_prepare_started = Instant::now();
        self.dynamic_model_renderer.entity_sun_relight = self.entity_sun_relight();
        self.sync_entity_cloud_shadow_sun();
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
        );
        self.ensure_prepared_wireframe_pipelines(
            prepared_grass.as_ref(),
            prepared_surface_sprite_effects.as_ref(),
        );
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
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
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
        let crosshair_world = late_view_sample
            .map(|measurement| measurement.crosshair_world)
            .unwrap_or(dynamic_crosshair_world);
        self.rebuild_player_name_tail(view_proj, crosshair_world);

        let encode_started = Instant::now();
        // Profiler hooks are no-ops unless r_gpuTimings is on; they let the
        // baseline's world/post passes be timed directly against the unified path.
        self.gpu_profiler.begin_frame(&self.device);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA fast baseline frame encoder"),
            });
        self.gpu_profiler
            .write_encoder_timestamp(&mut encoder, GpuPass::Frame, false);
        let use_gamma_post = self.frame_plan.use_post;
        let menu_backdrop_active = self.menu_backdrop_requested() && self.menu_backdrop.is_some();
        let presentation_output = if menu_backdrop_active {
            &self
                .menu_backdrop
                .as_ref()
                .expect("menu backdrop resources must exist while active")
                .view
        } else {
            &frame_view
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

        if let Some(world) = &self.world {
            let (color_view, resolve_target) = if let Some(msaa_view) = &self.targets.msaa_view {
                (msaa_view, Some(scene_output))
            } else {
                (scene_output, None)
            };

            let active = active_batch_selection(world, self.pvs_mode);
            let effective_area_mask = effective_area_mask(world, self.pvs_mode, area_mask);
            info.world_pvs_batches = u32::try_from(active.indices.len()).unwrap_or(u32::MAX);

            self.gpu_profiler
                .write_encoder_timestamp(&mut encoder, GpuPass::World, false);
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
            pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
            pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            let mut current_pipeline = None;
            let mut grass_drawn = false;
            let mut snow_shell_drawn = false;
            for &batch_index in active.indices {
                let batch = &active.batches[batch_index];
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
                            &mut current_pipeline,
                        );
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
                            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                            pass.set_vertex_buffer(
                                1,
                                world.legacy_dlight_surface_id_buffer.slice(..),
                            );
                            pass.set_index_buffer(
                                world.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                            current_pipeline = None;
                        }
                    }
                    grass_drawn = true;
                }
                if current_pipeline != Some(batch.source.pipeline) {
                    pass.set_pipeline(&world.fast_pipelines[&batch.source.pipeline]);
                    current_pipeline = Some(batch.source.pipeline);
                }
                pass.set_bind_group(1, &batch.fast_bind_group, &[]);
                draw_world_batch(&mut pass, batch, 0..1, self.ocean_clipmap_quality());
            }
            if !snow_shell_drawn {
                draw_snow_shell_fast(
                    &mut pass,
                    world,
                    view_proj,
                    snow_deform_center,
                    &mut current_pipeline,
                );
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
                    pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                    pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
                }
            }
            if let Some(prepared) = prepared_surface_sprite_effects.as_ref() {
                self.surface_sprite_effect_renderer.draw(
                    &mut pass,
                    &self.camera_bind_group,
                    &world.surface_sprite_effects,
                    prepared,
                );
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
            self.gpu_profiler
                .write_render_pass_timestamp(&mut pass, GpuPass::DynamicModels, false);
            self.dynamic_model_renderer
                .draw(&mut pass, &self.camera_bind_group);
            self.gpu_profiler
                .write_render_pass_timestamp(&mut pass, GpuPass::DynamicModels, true);

            // DynamicModelRenderer::draw() binds its own transient entity vertex/index
            // buffers. Everything below this point (wireframe, surface inspector, PVS
            // debug) addresses BSP/world index ranges, so restore the world geometry
            // bindings unconditionally before any of those debug draws run.
            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
            pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

            if self.wireframe_mask != 0 {
                let mask = self.wireframe_mask;
                if mask & (ui::wireframe::MAP | ui::wireframe::ENTITIES) != 0 {
                    if let Some(pipeline) = &self.wireframe_pipeline {
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &self.camera_bind_group, &[]);
                        for &batch_index in active.indices {
                            let batch = &active.batches[batch_index];
                            let category_enabled = if batch.is_inline_entity {
                                mask & ui::wireframe::ENTITIES != 0
                            } else {
                                mask & ui::wireframe::MAP != 0
                            };
                            if category_enabled
                                && (!self.ocean_enabled || batch.ocean_clipmap.is_none())
                                && batch.source.pipeline.class != DrawClass::Sky
                                && aabb_intersects_clip_frustum(
                                    batch.bounds_min,
                                    batch.bounds_max,
                                    view_proj,
                                )
                            {
                                draw_world_batch(&mut pass, batch, 0..1, None);
                            }
                        }
                    }
                }

                if mask & ui::wireframe::DEFORMATION != 0 {
                    if let Some(pipeline) = &self.fast_world_wireframe_pipeline {
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &self.fast_camera_bind_group, &[]);
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
                                pass.set_bind_group(1, &batch.fast_bind_group, &[]);
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
                pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            }
            if self.debug_volumes.draw(&mut pass, &self.camera_bind_group) {
                // The overlay bound its own buffers; the inspector below addresses BSP ranges.
                pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            }
            if let (Some(vertices), Some(pipeline)) = (
                &self.inspector_vertex_range,
                self.surface_inspector_pipeline.as_ref(),
            ) {
                if let Some(indices) = inspector_index_range(active.batches, vertices) {
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &self.camera_bind_group, &[]);
                    pass.draw_indexed(indices, 0, 0..1);
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
        if use_gamma_post {
            self.gpu_profiler
                .write_encoder_timestamp(&mut encoder, GpuPass::Post, false);
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
                pass.set_pipeline(&self.gamma_post_pipeline);
                pass.set_bind_group(0, &self.gamma_post_bind_group, &[]);
            } else {
                // Preserve the original smaller shader/bind group for gamma
                // alone. No grading code runs on the maximum-FPS baseline.
                pass.set_pipeline(&self.fast_post_pipeline);
                pass.set_bind_group(0, &self.fast_post_bind_group, &[]);
            }
            pass.draw(0..3, 0..1);
            drop(pass);
            self.gpu_profiler
                .write_encoder_timestamp(&mut encoder, GpuPass::Post, true);
        }

        // Deferred to `submit_ui_overlay` whenever a menu is drawn over the
        // frame, so the readout ends up above it rather than beneath.
        if !menu_backdrop_active
            && !self.egui_active
            && (self.screen_fx_vertex_count != 0
                || self.ui_vertex_count != 0
                || self.ui_dynamic_vertex_count != 0
                || self.ui_transient_vertex_count != 0)
        {
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

        if !menu_backdrop_active && !self.egui_active {
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
        let companion_scene_frame = companion_scene
            .and_then(|view| self.encode_companion_scene(&mut encoder, view, area_mask));
        self.gpu_profiler
            .write_encoder_timestamp(&mut encoder, GpuPass::Frame, true);
        self.gpu_profiler.resolve(&mut encoder);
        let command_buffer = encoder.finish();
        let profiler_readback_copy = self.gpu_profiler.encode_readback_copy(&self.device);
        info.cpu_encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;
        let submit_started = Instant::now();
        info.submit_call_at = Some(submit_started);
        let frame_submission = if let Some(readback_copy) = profiler_readback_copy {
            self.queue.submit([command_buffer, readback_copy])
        } else {
            self.queue.submit([command_buffer])
        };
        if let Some(scene_frame) = companion_scene_frame {
            self.companion_scene_ready.push(scene_frame);
        }
        if let Some(readback) = screenshot_readback
            .as_mut()
            .filter(|readback| readback.inline_copy)
        {
            readback.submission_index = Some(frame_submission.clone());
        }
        if menu_backdrop_active {
            self.submit_menu_backdrop(&frame_view);
        }
        self.submit_egui(&frame_view);
        if menu_backdrop_active || self.egui_active {
            self.submit_ui_overlay(&frame_view);
        }
        if let Some(readback) = screenshot_readback
            .as_mut()
            .filter(|readback| !readback.inline_copy)
        {
            self.submit_screenshot_copy(&frame.texture, readback);
        }
        info.cpu_submit_ms = submit_started.elapsed().as_secs_f64() * 1000.0;
        let present_started = Instant::now();
        self.window.pre_present_notify();
        frame.present();
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
