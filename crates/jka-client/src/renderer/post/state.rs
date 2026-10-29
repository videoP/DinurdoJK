//! Post state.
use crate::renderer::{
    cloud_wind, create_auto_exposure_bind_group, create_bloom_bind_groups, create_bloom_pipeline,
    create_cloud_resolve_bind_group, create_dof_bind_group, create_fast_post_bind_group,
    create_gamma_post_bind_group, create_post_bind_group, create_post_pipeline, create_post_pipeline_for,
    create_cloud_shadow_opaque_pipeline,
    create_rain_post_pipeline,
    create_ssao_temporal_bind_groups, create_ssr_temporal_bind_groups, pipeline_hash,
    CloudLayerSettings, CloudRenderSettings, CloudSunSettings, GlowMode, Mat4, PipelineJobKey,
    PostAaSettings, PostCameraFxSettings, PostCloudShadowSettings, PostCloudShapingSettings,
    PostCloudSkyAmbientSettings, PostCloudTemporalSettings, PostCloudTemporalTuningSettings,
    PostCloudVariationSettings, PostCloudWindSettings, PostColorSettings, PostFilmSettings,
    PostGrainSettings, PostSceneSettings, PostUniform, Renderer, SsaoTemporalUniform,
    SsrTemporalUniform, Vec3, WeatherOcclusionCache, CLOUD_INTERLEAVE_GRID, NO_WATER_SURFACE,
};
use bytemuck::Zeroable;

impl Renderer {
    /// Volumetric fog reads the linear-depth prepass, which has no depth for
    /// water seen over no receiver (the rendered ocean is not in the prepass).
    /// Post intersects those pixels' rays with the promoted water plane
    /// instead. Returns [plane Y, square half-extent (0 = off), centre X, Z].
    fn ocean_void_fog_params(&self, eye_y: f32) -> [f32; 4] {
        let off = [NO_WATER_SURFACE, 0.0, 0.0, 0.0];
        if !self.ocean_render_ready() || !self.ocean_sky_extend {
            return off;
        }
        let (Some(world), Some(ocean)) = (self.world.as_ref(), self.ocean.as_ref()) else {
            return off;
        };
        let mut best = off;
        for (slot, region) in world.ocean_sky_regions.iter().enumerate() {
            let (Some(_), Some(surface)) = (region, world.ocean_surfaces.get(slot)) else { continue };
            let plane = surface.plane_height;
            if plane >= eye_y || (best[1] > 0.0 && plane <= best[0]) {
                continue;
            }
            let Some((centre, radius)) = ocean.promoted_coverage(slot) else { continue };
            let extent = if self.ocean_sky_infinite { 1.0e9 } else { radius };
            best = [plane, extent, centre[0], centre[1]];
        }
        best
    }

    /// Queue only the active extraction variant and the shared downsample PSO.
    /// Off (without halation) does no polling or worker requests. Retain compiled
    /// PSOs for later toggles; never wait for shader compilation on a frame.
    pub(in crate::renderer) fn ensure_bloom_pipelines(&mut self) {
        let (slot, entry, extraction) = match self.glow_mode {
            GlowMode::Off if !self.halation_enabled => return,
            GlowMode::Off | GlowMode::ModernBloom => (
                0,
                "fs_extract_modern",
                &mut self.bloom_extract_modern_pipeline,
            ),
            GlowMode::JkaAuthored if self.halation_enabled => (
                2,
                "fs_extract_authored_halation",
                &mut self.bloom_extract_authored_halation_pipeline,
            ),
            GlowMode::JkaAuthored => (
                1,
                "fs_extract_authored",
                &mut self.bloom_extract_authored_pipeline,
            ),
            GlowMode::JkaAndBloom => (
                3,
                "fs_extract_combined",
                &mut self.bloom_extract_combined_pipeline,
            ),
        };
        for (slot, entry, pipeline) in [
            (slot, entry, extraction),
            (4, "fs_downsample", &mut self.bloom_downsample_pipeline),
        ] {
            if pipeline.is_some() {
                continue;
            }
            // Bloom PSOs always render to single-sample Rgba16Float; scene format
            // and viewport changes do not invalidate these cached variants.
            let key = PipelineJobKey::new("bloom", slot, 0);
            if let Some(ready) = self.pipeline_jobs.take_ready(key) {
                *pipeline = Some(ready);
                continue;
            }
            let device = self.device.clone();
            let layout = self.bloom_pipeline_layout.clone();
            self.pipeline_jobs.request(key, entry, move || {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("JKA lazy glow/bloom shader"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("../../bloom.wgsl").into()),
                });
                create_bloom_pipeline(&device, &layout, &shader, entry)
            });
        }
    }

    /// Polls/queues the full post pipeline without ever waiting for compilation.
    /// Until it is ready, the frame recorder falls back to the already-hot gamma
    /// copy so a first-use shader compile cannot hitch gameplay.
    pub(in crate::renderer) fn ensure_post_pipeline(&mut self, needed_now: bool) {
        // Full post is genuinely lazy: compact gamma/LUT/HDR frames must not
        // request or poll this heavyweight pipeline until an effect needs it.
        if !needed_now || self.post_pipeline.is_some() {
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

    /// No thermal-specific PSO is requested or polled while r_thermalVision=0.
    /// Both pipelines are cached across toggles. RT-TAA's alternative renderer
    /// is bypassed while thermal is active so there is always a thermal output.
    pub(in crate::renderer) fn ensure_thermal_post_pipeline(&mut self) {
        if self.thermal_vision == 0 { return; }
        let device = self.device.clone();
        let layout = self.post_pipeline_layout.clone();
        let shader = self.post_shader.clone();
        let output_format = self.config.format;
        // Only active thermal frames allocate these PSOs.  Encode BSP material
        // thermal class into the *existing* policy.b channel, no new MRT.
        for masked in [false, true] {
            let ready = if masked { self.thermal_depth_mask_pipeline.is_some() }
                        else { self.thermal_depth_pipeline.is_some() };
            if ready { continue; }
            let key = PipelineJobKey::new(if masked { "thermal-depth-mask" } else { "thermal-depth" }, 0, 0);
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                if masked { self.thermal_depth_mask_pipeline = Some(pipeline); }
                else { self.thermal_depth_pipeline = Some(pipeline); }
                continue;
            }
            let device = self.device.clone();
            let cam = self.camera_layout.clone();
            let surf = self.surface_layout.clone();
            let planar = self.planar_reflection_layout.clone();
            self.pipeline_jobs.request(key, if masked { "thermal masked world prepass" } else { "thermal world prepass" }, move || {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("JKA lazy thermal world classifier"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("../../depth_prepass.wgsl").into()),
                });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("JKA lazy thermal world prepass layout"),
                    bind_group_layouts: &[Some(&cam), Some(&surf), Some(&planar)],
                    immediate_size: 0,
                });
                if masked {
                    crate::renderer::create_depth_prepass_mask_pipeline(&device, &layout, &module, true)
                } else {
                    crate::renderer::create_depth_prepass_pipeline(&device, &layout, &module, true)
                }
            });
        }
        if self.thermal_post_pipeline.is_none() {
            let key = PipelineJobKey::new("thermal-post", 0, pipeline_hash(&(output_format,)));
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.thermal_post_pipeline = Some(pipeline);
            } else {
                let d = device.clone();
                let l = layout.clone();
                let s = shader.clone();
                self.pipeline_jobs.request(key, "thermal post", move || {
                    super::pipelines::create_post_pipeline_with_entry(
                        &d, &l, &s, output_format, None, "fs_main_thermal")
                });
            }
        }
        if self.taa_enabled && self.thermal_taa_pipeline.is_none() {
            let scene_format = self.scene_format();
            let key = PipelineJobKey::new("thermal-taa", 0,
                pipeline_hash(&(output_format, scene_format)));
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.thermal_taa_pipeline = Some(pipeline);
            } else {
                self.pipeline_jobs.request(key, "thermal TAA post", move || {
                    super::pipelines::create_taa_post_pipeline_with_entry(
                        &device, &layout, &shader, output_format, scene_format, "fs_main_taa_thermal")
                });
            }
        }
    }

    /// Specialized opaque-only cloud shadow pass, queued on the normal shader
    /// worker. No compile, PSO, or draw overhead with cloud shadows disabled.
    pub(in crate::renderer) fn ensure_cloud_shadow_opaque_pipeline(&mut self) {
        if !self.clouds_enabled || !self.cloud_shadows_enabled
            || self.cloud_shadow_opaque_pipeline.is_some()
        {
            return;
        }
        let format = self.scene_format();
        let samples = self.msaa_samples;
        let key = PipelineJobKey::new(
            "cloud-shadow-opaque", 0, pipeline_hash(&(format, samples)),
        );
        if let Some(ready) = self.pipeline_jobs.take_ready(key) {
            self.cloud_shadow_opaque_pipeline = Some(ready);
            return;
        }
        let (device, layout, shader) = (
            self.device.clone(),
            self.post_pipeline_layout.clone(),
            self.post_shader.clone(),
        );
        self.pipeline_jobs.request(key, "cloud shadow (before water)", move || {
            create_cloud_shadow_opaque_pipeline(&device, &layout, &shader, format, samples)
        });
    }

    /// r_postSpecialize: the full-post pipeline compiled for `mask`, or None
    /// while it compiles on the pipeline workers (the caller then draws with the
    /// general pipeline, which renders the same image).
    pub(in crate::renderer) fn ensure_post_specialized(&mut self, mask: u32) -> bool {
        if !self.post_specialize || self.post_specialized.contains_key(&(self.config.format, mask)) {
            return self.post_specialize;
        }
        let key = PipelineJobKey::new("post-specialized", mask, pipeline_hash(&(self.config.format, mask)));
        if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
            self.post_specialized.insert((self.config.format, mask), pipeline);
            return true;
        }
        let (device, layout, shader, format) = (
            self.device.clone(),
            self.post_pipeline_layout.clone(),
            self.post_shader.clone(),
            self.config.format,
        );
        self.pipeline_jobs.request(key, "specialized full post", move || {
            create_post_pipeline_for(&device, &layout, &shader, format, Some(mask))
        });
        true
    }

    /// Polls/queues the specialized rain-only fullscreen post pipeline.
    pub(in crate::renderer) fn ensure_rain_post_pipeline(&mut self, needed_now: bool) {
        if !needed_now || self.rain_post_pipeline.is_some() {
            return;
        }
        let key = PipelineJobKey::new("post-rain", 0, pipeline_hash(&(self.config.format,)));
        if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
            self.rain_post_pipeline = Some(pipeline);
            return;
        }
        let (device, layout, shader, format) = (
            self.device.clone(),
            self.post_pipeline_layout.clone(),
            self.post_shader.clone(),
            self.config.format,
        );
        self.pipeline_jobs.request(key, "rain-only post", move || {
            create_rain_post_pipeline(&device, &layout, &shader, format)
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
        let _ = self.write_post_uniform(
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            0.0,
            [0.0; 4],
            false,
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
        dof_frame_active: bool,
    ) -> u32 {
        let (cloud_blades, cloud_blade_count) = self.cloud_foreground_uniform(
            view_proj,
            Vec3::new(camera_pos_time[0], camera_pos_time[1], camera_pos_time[2]),
        );
        let ocean_void_fog = self.ocean_void_fog_params(camera_pos_time[1]);
        let legacy1_global_fog = self.weather.fog.legacy1_global_post_params();
        let (legacy_fog, legacy_fog_scale) = legacy1_global_fog
            .map_or(([0.0; 4], 0.0), |(color, depth, scale)| {
                ([color[0], color[1], color[2], depth], scale)
            });
        let uniform = PostUniform {
            color: PostColorSettings {
                gamma: self.output_gamma(),
                // The floating-point scene format and display tone mapping are
                // independent: 0 off, 0..1 SDR ACES, 3 HDR output, 4 upstream
                // Blue Water SDR display grade + AgX.
                tone_mapping: if self.hdr_output.is_some() { 3.0 } else if self.native_display_mapping_active() {
                    4.0
                } else {
                    self.tone_mapping_strength
                },
                bloom: if self.glow_mode.enabled() { 1.0 } else { 0.0 },
                ssao: if self.ssao_enabled { 1.0 } else { 0.0 },
            },
            aa: PostAaSettings {
                fxaa: if self.fxaa_enabled { 1.0 } else { 0.0 },
                viewport_width: self.config.width.max(1) as f32,
                viewport_height: self.config.height.max(1) as f32,
                taa: if self.taa_enabled { 1.0 } else if self.rt_taa_active() { 1.0 + self.rt_taa_mode as f32 } else { 0.0 },
            },
            taa_params: [
                if self.hdr_enabled || self.native_atmosphere_active() || self.blue_water_environment.weather_backend == 1 { 1.0 } else { 0.0 },
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
                volumetric_fog: if self.weather_froxels_needed() { 1.0 } else { 0.0 },
                // r_ssrDebug rides along as a magnitude, exactly as
                // r_contactShadowDebug does above: every existing consumer only
                // tests `> 0.5` for "SSR is on".
                ssr: if self.ssr_enabled {
                    1.0 + f32::from(self.ssr_debug)
                } else {
                    0.0
                },
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
                time: self.frame_time(),
                exposure: self.color_grade.exposure_ev,
            },
            exposure_controls: [self.effective_auto_exposure_strength(),
                if self.native_display_mapping_active() { 1.0 } else { 0.0 }, f32::from(self.thermal_vision),
                if self.thermal_vision == 0 { 0.0 } else {
                    let eye = crate::scene::jka_position([camera_pos_time[0], camera_pos_time[1], camera_pos_time[2]]);
                    let contents = self.world.as_ref().map_or(0, |world| {
                        world.thermal_liquids.iter().filter(|liquid| liquid.contains(eye))
                            .map(|liquid| liquid.kind).max().unwrap_or(0)
                    });
                    if contents == 0 && self.ocean_render_ready()
                        && self.camera_water_surface > NO_WATER_SURFACE { 1.0 }
                    else { f32::from(contents) }
                }],
            camera_fx: PostCameraFxSettings {
                motion_blur_scale,
                depth_of_field: if dof_frame_active { self.depth_of_field_strength } else { 0.0 },
                dof_focus_distance: self.dof_focus_distance,
                dof_quality: self.dof_quality.shader_value(),
            },
            legacy_fog,
            clouds: CloudRenderSettings {
                values: [
                    if self.clouds_enabled && self.cloud_backend == 0 { 1.0 } else { 0.0 },
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
                    if self.clouds_enabled && self.cloud_shadows_enabled && self.cloud_backend == 0 {
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
                if self.weather.rain.enabled && !self.weather.rain.fast_post { 1.0 } else { 0.0 },
                self.weather.rain.intensity.shader_value(),
                if self.weather.rain.fast_post {
                    0.0
                } else {
                    self.weather.rain.intensity.haze_strength()
                },
                if self.weather.rain.fast_post {
                    0.0
                } else {
                    self.weather.rain.puddle_amount
                },
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
                if self.weather.rain.fast_post { 0.0 } else { self.weather.rain.grade_amount() },
                if self.weather.rain.puddle_quality.is_high() {
                    1.0
                } else {
                    0.0
                },
                self.weather.rain.lens_drop_amount(),
            ],
            legacy_weather: [
                self.weather.atmosphere.sand_strength(),
                self.weather.atmosphere.space_strength(),
                self.weather.rain.acid_strength,
                if self.precipitation_2d_haze_active() { 1.0 } else { 0.0 },
            ],
            // A single 2D precipitation shader handles rain and snow. Rain's
            // fast_post A/B option must not turn off snow's independent haze.
            snow_haze: [
                if self.weather_uses_2d_haze() && self.weather.snow.enabled {
                    self.weather.snow.density_scale.clamp(0.0, 5.0)
                } else {
                    0.0
                },
                0.0,
                0.0,
                0.0,
            ],
            // The optics only exist with the GPU ocean; a plain BSP water
            // shader gets no underwater treatment, so clouds are left alone.
            underwater: [
                if self.ocean_render_ready() && self.camera_water_surface > NO_WATER_SURFACE {
                    // Depth below the REAL surface over the eye (0 offset = flat plane).
                    self.camera_water_surface + self.eye_wave_offset
                } else if self.ocean_render_ready() {
                    self.camera_water_surface
                } else {
                    NO_WATER_SURFACE
                },
                self.ocean_settings.optics.absorption_distance(),
                ocean_void_fog[0],
                ocean_void_fog[1],
            ],
            cloud_foreground: [
                cloud_blade_count,
                ocean_void_fog[2],
                ocean_void_fog[3],
                self.portal_depth.inverse_scale,
            ],
            cloud_blades,
            camera_pos_time,
            prev_camera_pos_time: if self.cloud_history_valid || (self.rt_taa_active() && history_valid) {
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
        post_feature_mask(&uniform)
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
                // r_ssrDebug 6 holds the temporal accumulation permanently
                // invalid, so every frame is a fresh trace. The blend weight is
                // 0.90 while the camera is still and falls away with motion, so
                // this is the only way to see a standing-still frame without it.
                // 7 does the same: blending flat outcome labels across frames
                // would average them into colours that name no outcome.
                if self.ssr_history_valid
                    && self.ssr_debug != 6
                    && self.ssr_debug != 7
                    && self.ssr_debug != 9
                {
                    1.0
                } else {
                    0.0
                },
                (self.ssr_frame_index & 0xffff) as f32,
            ],
            camera_pos_time,
            previous_camera_pos: [
                previous_camera.x,
                previous_camera.y,
                previous_camera.z,
                // Quality is 0..4, so the debug selector rides above it in the
                // same slot rather than growing the uniform: quality | debug << 3.
                (self.reflection_quality.shader_value() as f32)
                    + 8.0 * f32::from(self.ssr_debug),
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
            self.targets.authored_glow.as_ref().map(|target| &target.view),
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
                            self.portal_depth.view(),
                        )
                    })
                })
            })
        });
        // Unrelated history texture stands in for every UNUSED post scene
        // sampler. Binding self.targets.scene_view in a pass writing to that
        // texture would violate wgpu attachment usage even if code ignores it.
        let history = &self.targets.history_views[0];
        let noise = self.cloud_noise_resources.as_ref().unwrap_or(&self.cloud_noise_fallback);
        self.cloud_shadow_opaque_bind_group = create_post_bind_group(
            &self.device,
            &self.post_layout,
            history,
            &self.targets.linear_depth_view,
            history,
            history,
            history,
            &self.targets.linear_depth_view,
            &self.post_sampler,
            &self.post_buffer,
            &self.weather.fog.resources.buffer,
            self.targets.bloom.as_ref(),
            self.targets.dof.as_ref(),
            &self.color_lut_view,
            &self.color_lut_sampler,
            &noise.detail_view,
            &noise.sampler,
            &self.targets.cloud_transfer_views[0],
            self.weather.rain.collision_view(),
            &self.targets.rain_haze_mask_view,
            &noise.weather_view,
            &self.targets.motion_vector_view,
            &self.targets.ao_depth_view,
            &self.targets.reflection_mask_view,
            &self.auto_exposure_state_buffer,
            self.targets.ssr_visibility_view.as_ref().unwrap_or(&self.white.view),
            self.portal_depth.view(),
        );
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

/// `POST_FEATURES` bits (post.wgsl `fs_main`) for a uniform. A bit is set when
/// any input of its branch is non-zero (NaN included), a superset of the
/// shader's own `> 0.5` / `> 0.001` tests, so a cleared bit only removes a
/// branch that would have returned its input unchanged.
pub(in crate::renderer) fn post_feature_mask(uniform: &PostUniform) -> u32 {
    let on = |value: f32| value != 0.0;
    let mut mask = 0u32;
    let mut set = |bit: u32, enabled: bool| if enabled { mask |= bit };
    set(1, on(uniform.aa.fxaa));
    set(2, on(uniform.camera_fx.motion_blur_scale));
    set(4, on(uniform.camera_fx.depth_of_field));
    set(8, on(uniform.rain[3]) || on(uniform.scene.ssr) || on(uniform.scene.contact_shadows)
        || on(uniform.color.ssao) || on(uniform.scene.volumetric_fog) || on(uniform.legacy_weather[3]));
    set(16, on(uniform.clouds.values[0]));
    set(32, uniform.legacy_weather[..3].iter().any(|&value| on(value)));
    set(64, on(uniform.color.bloom));
    set(128, on(uniform.film.halation));
    set(256, on(uniform.exposure_controls[1]));
    set(512, on(uniform.taa_params[3]));
    set(1024, on(uniform.taa_params[1]));
    mask
}
