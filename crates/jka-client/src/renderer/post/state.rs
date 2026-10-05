//! Post state.
use crate::renderer::{
    cloud_wind, create_auto_exposure_bind_group, create_bloom_bind_groups,
    create_cloud_resolve_bind_group, create_dof_bind_group, create_fast_post_bind_group,
    create_gamma_post_bind_group, create_post_bind_group, create_post_pipeline,
    create_ssao_temporal_bind_groups, create_ssr_temporal_bind_groups, pipeline_hash,
    CloudLayerSettings, CloudRenderSettings, CloudSunSettings, Mat4, PipelineJobKey,
    PostAaSettings, PostCameraFxSettings, PostCloudShadowSettings, PostCloudShapingSettings,
    PostCloudSkyAmbientSettings, PostCloudTemporalSettings, PostCloudTemporalTuningSettings,
    PostCloudVariationSettings, PostCloudWindSettings, PostColorSettings, PostFilmSettings,
    PostGrainSettings, PostSceneSettings, PostUniform, Renderer, SsaoTemporalUniform,
    SsrTemporalUniform, Vec3, WeatherOcclusionCache, CLOUD_INTERLEAVE_GRID, NO_WATER_SURFACE,
};
use bytemuck::Zeroable;

impl Renderer {
    /// Polls/queues the full post pipeline without ever waiting for compilation.
    /// Until it is ready, the frame recorder falls back to the already-hot gamma
    /// copy so a first-use shader compile cannot hitch gameplay.
    pub(in crate::renderer) fn ensure_post_pipeline(&mut self, _needed_now: bool) {
        if self.post_pipeline.is_some() {
            return;
        }
        let key = PipelineJobKey::new("post", 0, pipeline_hash(&(self.config.format,)));
        if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
            self.post_pipeline = Some(pipeline);
            return;
        }
        let (device, layout, shader, format) = (
            self.device.clone(),
            self.post_pipeline_layout.clone(),
            self.post_shader.clone(),
            self.config.format,
        );
        self.pipeline_jobs.request(key, "full post", move || {
            create_post_pipeline(&device, &layout, &shader, format)
        });
    }

    /// SMAA's third-party target bundles its render pipelines and intermediate
    /// textures. Construct the whole target on the shared pipeline workers so
    /// selecting SMAA never compiles its PSOs inline on a gameplay frame. Until
    /// ready, the frame simply presents without SMAA.
    pub(in crate::renderer) fn ensure_smaa_target(&mut self) {
        if self.smaa_target.is_some() {
            return;
        }
        let width = self.config.width.max(1);
        let height = self.config.height.max(1);
        let format = self.config.format;
        let key = PipelineJobKey::new("smaa-target", 0, pipeline_hash(&(width, height, format)));
        if let Some(target) = self.pipeline_jobs.take_ready::<smaa::SmaaTarget>(key) {
            self.smaa_target = Some(target);
            return;
        }
        let device = self.device.clone();
        let queue = self.queue.clone();
        self.pipeline_jobs
            .request(key, "SMAA target + pipelines", move || {
                smaa::SmaaTarget::new(
                    &device,
                    &queue,
                    width,
                    height,
                    format,
                    smaa::SmaaMode::Smaa1X,
                )
            });
    }

    pub(in crate::renderer) fn update_post_uniform(&self) {
        self.write_post_uniform(
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            0.0,
            [0.0; 4],
            false,
        );
    }

    pub(in crate::renderer) fn write_post_uniform(
        &self,
        view_proj: Mat4,
        inv_view_proj: Mat4,
        cloud_inv_view_proj: Mat4,
        cloud_prev_view_proj: Mat4,
        prev_view_proj: Mat4,
        motion_prev_view_proj: Mat4,
        motion_blur_scale: f32,
        camera_pos_time: [f32; 4],
        history_valid: bool,
    ) {
        let (cloud_blades, cloud_blade_count) = self.cloud_foreground_uniform(
            view_proj,
            Vec3::new(camera_pos_time[0], camera_pos_time[1], camera_pos_time[2]),
        );
        let legacy1_global_fog = self.weather.fog.legacy1_global_post_params();
        let (legacy_fog, legacy_fog_scale) = legacy1_global_fog
            .map_or(([0.0; 4], 0.0), |(color, depth, scale)| {
                ([color[0], color[1], color[2], depth], scale)
            });
        let uniform = PostUniform {
            color: PostColorSettings {
                gamma: self.output_gamma(),
                // The floating-point scene format and display tone mapping are
                // independent. This lane controls ACES only; HDR rendering is
                // selected separately by scene_format().
                tone_mapping: if self.tone_mapping_enabled { 1.0 } else { 0.0 },
                bloom: if self.bloom_enabled { 1.0 } else { 0.0 },
                ssao: if self.ssao_enabled { 1.0 } else { 0.0 },
            },
            aa: PostAaSettings {
                fxaa: if self.fxaa_enabled { 1.0 } else { 0.0 },
                viewport_width: self.config.width.max(1) as f32,
                viewport_height: self.config.height.max(1) as f32,
                taa: if self.taa_enabled { 1.0 } else { 0.0 },
            },
            taa_params: [
                if self.hdr_enabled { 1.0 } else { 0.0 },
                if self.reflection_debug_enabled {
                    1.0
                } else {
                    0.0
                },
                self.reflection_quality.shader_value() as f32,
                legacy_fog_scale,
            ],
            scene: PostSceneSettings {
                // Contact shadows march toward the sun, so a map with no authored
                // sun (and no override) has nothing to march toward.
                contact_shadows: if self.contact_shadows_enabled
                    && (self.sun_override
                        || self.world.as_ref().is_some_and(|world| world.sun.is_some()))
                {
                    // r_contactShadowDebug shows the pass's per-pixel decision
                    // instead of the scene.
                    1.0 + f32::from(self.contact_shadow_debug)
                } else {
                    0.0
                },
                volumetric_fog: if self.weather.fog.volumetric_effective()
                    || self.weather.rain.enabled
                {
                    1.0
                } else {
                    0.0
                },
                ssr: if self.ssr_enabled { 1.0 } else { 0.0 },
                history_valid: if history_valid { 1.0 } else { 0.0 },
            },
            film: PostFilmSettings {
                halation: if self.halation_enabled { 1.0 } else { 0.0 },
                chromatic_aberration: self.chromatic_aberration_strength,
                vignette: if self.vignette_enabled { 1.0 } else { 0.0 },
                lut_strength: self.color_lut_effective_strength,
            },
            grain: PostGrainSettings {
                strength: self.film_grain_strength,
                grain_size: 1.35,
                time: self.started.elapsed().as_secs_f32(),
                exposure: 0.0,
            },
            camera_fx: PostCameraFxSettings {
                motion_blur_scale,
                depth_of_field: self.depth_of_field_strength,
                dof_focus_distance: self.dof_focus_distance,
                dof_quality: self.dof_quality.shader_value(),
            },
            legacy_fog,
            clouds: CloudRenderSettings {
                values: [
                    if self.clouds_enabled { 1.0 } else { 0.0 },
                    self.cloud_type.shader_value(),
                    self.cloud_quality,
                    self.cloud_coverage,
                ],
            },
            cloud_layer: CloudLayerSettings {
                values: [
                    self.cloud_height,
                    self.cloud_thickness,
                    self.weather_wind.speed,
                    self.weather_wind.direction.to_radians(),
                ],
            },
            cloud_sun: {
                let sun = self.active_sun();
                CloudSunSettings {
                    direction_intensity: [
                        sun.direction[0],
                        sun.direction[1],
                        sun.direction[2],
                        sun.intensity,
                    ],
                    color_density: [sun.color[0], sun.color[1], sun.color[2], 1.0],
                }
            },
            cloud_shadow: PostCloudShadowSettings {
                values: [
                    if self.clouds_enabled && self.cloud_shadows_enabled {
                        1.0
                    } else {
                        0.0
                    },
                    0.75,
                    if self.cloud_temporal_depth_fix {
                        1.0
                    } else {
                        0.0
                    },
                    if self.cloud_terrain_interaction {
                        1.0
                    } else {
                        0.0
                    },
                ],
            },
            cloud_shaping: PostCloudShapingSettings {
                values: [
                    self.cloud_shear,
                    self.cloud_base_variation,
                    if self.cloud_empty_skip { 1.0 } else { 0.0 },
                    self.cloud_aerial,
                ],
            },
            cloud_sky_ambient: {
                // Falls back to the stored default until a map with a skybox is
                // loaded, so the menu backdrop and fog-only levels still light
                // clouds sensibly.
                let sky = self
                    .world
                    .as_ref()
                    .map_or(self.cloud_sky_average, |world| world.sky_average);
                PostCloudSkyAmbientSettings {
                    values: [
                        sky[0],
                        sky[1],
                        sky[2],
                        if self.cloud_sky_ambient_enabled {
                            1.0
                        } else {
                            0.0
                        },
                    ],
                }
            },
            cloud_temporal_tuning: PostCloudTemporalTuningSettings {
                values: [
                    self.cloud_history_blend,
                    self.cloud_motion_reject,
                    if self.cloud_history_depth_reject {
                        1.0
                    } else {
                        0.0
                    },
                    if self.cloud_shape_evolution { 1.0 } else { 0.0 },
                ],
            },
            cloud_variation: PostCloudVariationSettings {
                values: [
                    self.cloud_thickness_variation,
                    self.cloud_size,
                    self.weather_wind.gust,
                    self.weather_wind.shift.to_radians(),
                ],
            },
            cloud_temporal: PostCloudTemporalSettings {
                values: [
                    if self.clouds_enabled && self.cloud_temporal_enabled {
                        1.0
                    } else {
                        0.0
                    },
                    if self.cloud_history_valid { 1.0 } else { 0.0 },
                    (self.cloud_temporal_frame_index & 3) as f32,
                    CLOUD_INTERLEAVE_GRID as f32,
                ],
            },
            cloud_wind: if self.clouds_enabled {
                cloud_wind::terms(
                    cloud_wind::CloudWind {
                        angle: self.weather_wind.direction.to_radians(),
                        speed: self.weather_wind.speed,
                        gust: self.weather_wind.gust,
                        shift: self.weather_wind.shift.to_radians(),
                    },
                    camera_pos_time[3],
                    if self.cloud_history_valid {
                        self.previous_frame_time
                    } else {
                        camera_pos_time[3]
                    },
                )
                .into()
            } else {
                PostCloudWindSettings::zeroed()
            },
            rain: [
                if self.weather.rain.enabled { 1.0 } else { 0.0 },
                self.weather.rain.intensity.shader_value(),
                self.weather.rain.intensity.haze_strength(),
                self.weather.rain.puddle_amount,
            ],
            weather_occlusion: match &self.weather.rain.occlusion {
                WeatherOcclusionCache::Ready(info) => [
                    info.min_xz[0],
                    info.min_xz[1],
                    info.inv_extent_xz[0],
                    info.inv_extent_xz[1],
                ],
                WeatherOcclusionCache::Pending(_)
                | WeatherOcclusionCache::Building(_)
                | WeatherOcclusionCache::Unavailable => [0.0; 4],
            },
            weather_look: [
                self.weather.rain.puddle_scatter,
                self.weather.rain.grade_amount(),
                if self.weather.rain.puddle_quality.is_high() {
                    1.0
                } else {
                    0.0
                },
                0.0,
            ],
            // The optics only exist with the GPU ocean; a plain BSP water
            // shader gets no underwater treatment, so clouds are left alone.
            underwater: [
                if self.ocean_enabled {
                    self.camera_water_surface
                } else {
                    NO_WATER_SURFACE
                },
                self.ocean_settings.optics.absorption_distance(),
                0.0,
                0.0,
            ],
            cloud_foreground: [cloud_blade_count, 0.0, 0.0, 0.0],
            cloud_blades,
            camera_pos_time,
            prev_camera_pos_time: if self.cloud_history_valid {
                [
                    self.previous_camera_position.x,
                    self.previous_camera_position.y,
                    self.previous_camera_position.z,
                    self.previous_frame_time,
                ]
            } else {
                camera_pos_time
            },
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            cloud_inv_view_proj: cloud_inv_view_proj.to_cols_array_2d(),
            cloud_prev_view_proj: cloud_prev_view_proj.to_cols_array_2d(),
            prev_view_proj: prev_view_proj.to_cols_array_2d(),
            motion_prev_view_proj: motion_prev_view_proj.to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.post_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    pub(in crate::renderer) fn write_ssao_temporal_uniform(
        &self,
        view_proj: Mat4,
        inv_view_proj: Mat4,
        camera_pos_time: [f32; 4],
    ) {
        let previous_view_proj = if self.ssao_history_valid {
            self.previous_view_proj
        } else {
            view_proj
        };
        let previous_camera = if self.ssao_history_valid {
            self.previous_camera_position
        } else {
            Vec3::new(camera_pos_time[0], camera_pos_time[1], camera_pos_time[2])
        };
        let uniform = SsaoTemporalUniform {
            viewport_history: [
                self.config.width.max(1) as f32,
                self.config.height.max(1) as f32,
                if self.ssao_history_valid { 1.0 } else { 0.0 },
                0.0,
            ],
            camera_pos_time,
            previous_camera_pos: [previous_camera.x, previous_camera.y, previous_camera.z, 0.0],
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            prev_view_proj: previous_view_proj.to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.ssao_temporal_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    pub(in crate::renderer) fn write_ssr_temporal_uniform(
        &self,
        view_proj: Mat4,
        inv_view_proj: Mat4,
        camera_pos_time: [f32; 4],
    ) {
        let previous_view_proj = if self.ssr_history_valid {
            self.previous_view_proj
        } else {
            view_proj
        };
        let previous_camera = if self.ssr_history_valid {
            self.previous_camera_position
        } else {
            Vec3::new(camera_pos_time[0], camera_pos_time[1], camera_pos_time[2])
        };
        let uniform = SsrTemporalUniform {
            viewport_history: [
                self.config.width.max(1) as f32,
                self.config.height.max(1) as f32,
                if self.ssr_history_valid { 1.0 } else { 0.0 },
                (self.ssr_frame_index & 0xffff) as f32,
            ],
            camera_pos_time,
            previous_camera_pos: [
                previous_camera.x,
                previous_camera.y,
                previous_camera.z,
                self.reflection_quality.shader_value() as f32,
            ],
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            prev_view_proj: previous_view_proj.to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.ssr_temporal_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    pub(in crate::renderer) fn rebuild_post_bind_group(&mut self) {
        // The march target is recreated with the other cloud targets whenever
        // render resolution changes, so its bind group has to follow.
        self.cloud_resolve_bind_group = create_cloud_resolve_bind_group(
            &self.device,
            &self.cloud_resolve_layout,
            &self.targets.cloud_march_view,
        );
        self.auto_exposure_bind_group = create_auto_exposure_bind_group(
            &self.device,
            &self.auto_exposure_layout,
            &self.targets.scene_view,
            &self.auto_exposure_state_buffer,
            &self.auto_exposure_settings_buffer,
        );
        self.rebuild_color_lut_bind_groups();
        self.ssao_temporal_bind_groups = create_ssao_temporal_bind_groups(
            &self.device,
            &self.ssao_temporal_layout,
            &self.targets.linear_depth_view,
            self.targets.ssao_history.as_ref(),
            &self.post_sampler,
            &self.ssao_temporal_buffer,
        );
        self.ssr_temporal_bind_groups = create_ssr_temporal_bind_groups(
            &self.device,
            &self.ssr_temporal_layout,
            &self.targets.scene_view,
            &self.targets.linear_depth_view,
            self.targets.ssr_history.as_ref(),
            &self.post_sampler,
            &self.ssr_temporal_buffer,
            &self.shadow_resources.weather_height_view,
            &self.shadow_resources.weather_sampler,
            &self.shadow_resources.weather_surface_buffer,
            &self.targets.reflection_mask_view,
            self.targets.ssr_visibility_view.as_ref(),
        );
        self.bloom_bind_groups = create_bloom_bind_groups(
            &self.device,
            &self.bloom_layout,
            &self.post_sampler,
            &self.targets.scene_view,
            self.targets.bloom.as_ref(),
        );
        self.dof_bind_group = create_dof_bind_group(
            &self.device,
            &self.dof_layout,
            &self.targets.scene_view,
            &self.targets.linear_depth_view,
            &self.post_sampler,
            &self.dof_buffer,
            self.targets.dof.as_ref(),
        );
        self.fast_post_bind_group = create_fast_post_bind_group(
            &self.device,
            &self.fast_post_layout,
            &self.targets.scene_view,
            &self.post_sampler,
            &self.gamma_post_buffer,
        );
    }

    /// LUT view changes affect just the post samplers, not temporal history,
    /// bloom, DOF, cloud resolve, auto exposure, or the gamma-only baseline.
    pub(in crate::renderer) fn rebuild_color_lut_bind_groups(&mut self) {
        self.post_bind_groups = std::array::from_fn(|history_index| {
            std::array::from_fn(|ssao_index| {
                std::array::from_fn(|ssr_index| {
                    std::array::from_fn(|cloud_index| {
                        let ssao_view = self
                            .targets
                            .ssao_history
                            .as_ref()
                            .map_or(&self.targets.scene_view, |history| {
                                &history.views[ssao_index]
                            });
                        let (ssr_radiance_view, ssr_depth_view) =
                            self.targets.ssr_history.as_ref().map_or(
                                (&self.targets.scene_view, &self.targets.linear_depth_view),
                                |history| {
                                    (
                                        &history.radiance_views[ssr_index],
                                        &history.depth_views[ssr_index],
                                    )
                                },
                            );
                        let cloud_noise = self
                            .cloud_noise_resources
                            .as_ref()
                            .unwrap_or(&self.cloud_noise_fallback);
                        create_post_bind_group(
                            &self.device,
                            &self.post_layout,
                            &self.targets.scene_view,
                            &self.targets.linear_depth_view,
                            &self.targets.history_views[history_index],
                            ssao_view,
                            ssr_radiance_view,
                            ssr_depth_view,
                            &self.post_sampler,
                            &self.post_buffer,
                            &self.weather.fog.resources.buffer,
                            self.targets.bloom.as_ref(),
                            self.targets.dof.as_ref(),
                            &self.color_lut_view,
                            &self.color_lut_sampler,
                            &cloud_noise.detail_view,
                            &cloud_noise.sampler,
                            &self.targets.cloud_transfer_views[cloud_index],
                            self.weather.rain.collision_view(),
                            &self.targets.rain_haze_mask_view,
                            &cloud_noise.weather_view,
                            &self.targets.motion_vector_view,
                            &self.targets.ao_depth_view,
                            &self.targets.reflection_mask_view,
                            &self.auto_exposure_state_buffer,
                            self.targets
                                .ssr_visibility_view
                                .as_ref()
                                .unwrap_or(&self.white.view),
                        )
                    })
                })
            })
        });
        self.gamma_post_bind_group = create_gamma_post_bind_group(
            &self.device,
            &self.gamma_post_layout,
            &self.targets.scene_view,
            &self.post_sampler,
            &self.gamma_post_buffer,
            &self.color_lut_view,
            &self.color_lut_sampler,
        );
    }

    pub(in crate::renderer) fn rebuild_rain_haze_mask_bind_group(&mut self) {
        self.weather.rain.rebuild_haze_bind_group(
            &self.device,
            &self.post_buffer,
            &self.targets.linear_depth_view,
            &self.targets.rain_haze_mask_view,
        );
    }

    /// Flush any Queue::write_buffer/write_texture transfers queued during frame
    /// preparation when surface acquisition fails. On native wgpu these writes use
    /// fresh staging allocations that are retained until a queue submission finishes.
    /// Without this, an occluded/minimized window can repeatedly prepare a frame,
    /// skip the normal submit, and grow staging memory until the device is OOM.
    pub(in crate::renderer) fn flush_staged_queue_writes(&self) {
        self.queue.submit([]);
    }
}
