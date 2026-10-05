//! Settings graphics.
use crate::app::{
    scene, ui, App, CloudRenderResolution, CloudType, ColorLutPreset, CullDebugMode,
    DetailTextureMode, DofQuality, DynamicLightsMode, DynamicShadowsMode,
    EntityAmbientLightingMode, EntityShadowLight, FogMode, FootprintMode, FxGeometryMode,
    Ghoul2BatchMode, Ghoul2SkinningMode, Instant, PlanarReflectionDebugMode, PostEffects,
    PuddleQuality, PvsMode, RainIntensity, ReflectionQuality, RenderCommand, VsyncMode,
};

impl App {
    pub(in crate::app) fn set_gamma_method(&mut self, method: crate::gamma::GammaMethod) {
        self.video.gamma_method = method;
        self.sync_gamma_method();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_gamma(&mut self, gamma: f32) {
        self.video.gamma = gamma.clamp(0.5, 3.0);
        self.video.sync_linked_brightness();
        self.render_command(RenderCommand::SetGamma(self.video.gamma));
        if self.video.gamma_method == crate::gamma::GammaMethod::Hardware {
            self.gamma_slider_changed();
        }
        // Unlocked sliders are expressed relative to the master, so their
        // effective multiplier changes whenever the master moves.
        self.sync_brightness_scales();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn sync_debug_volumes(&mut self) {
        self.render_command(RenderCommand::SetDebugVolumes {
            triggers: self.video.draw_triggers,
            clips: self.video.draw_clip_brushes,
        });
        self.publish_ui();
    }

    /// `r_drawEntities`. Unlike `draw_triggers`/`draw_clip_brushes` this isn't
    /// pushed to the renderer here â€” the per-tick marker rebuild
    /// (`rebuild_entity_markers`) does that every frame while it's on â€” except
    /// when turning it off, where the overlay must be cleared immediately
    /// since nothing will send another update once the rebuild stops running.
    pub(in crate::app) fn set_draw_entities(&mut self, enabled: bool) {
        self.video.draw_entities = enabled;
        if !enabled {
            self.render_command(RenderCommand::SetEntityMarkers(None));
        }
        self.publish_ui();
    }

    /// Pushes the effective model / dynamic-light multipliers to the renderer.
    pub(in crate::app) fn sync_brightness_scales(&mut self) {
        self.render_command(RenderCommand::SetModelBrightness(
            self.video.effective_model_brightness(),
        ));
        self.render_command(RenderCommand::SetDynamicLightBrightness(
            self.video.effective_dynamic_light_brightness(),
        ));
    }

    pub(in crate::app) fn set_model_brightness(&mut self, value: f32) {
        self.video.model_brightness = value.clamp(0.5, 3.0);
        // Dragging a linked slider implicitly detaches it from the master.
        self.video.model_brightness_locked = false;
        self.sync_brightness_scales();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_model_brightness_locked(&mut self, locked: bool) {
        self.video.model_brightness_locked = locked;
        self.video.sync_linked_brightness();
        self.sync_brightness_scales();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_dynamic_light_brightness(&mut self, value: f32) {
        self.video.dynamic_light_brightness = value.clamp(0.5, 3.0);
        self.video.dynamic_light_brightness_locked = false;
        self.sync_brightness_scales();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_dynamic_light_brightness_locked(&mut self, locked: bool) {
        self.video.dynamic_light_brightness_locked = locked;
        self.video.sync_linked_brightness();
        self.sync_brightness_scales();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_chromatic_aberration_strength(&mut self, strength: f32) {
        self.video.chromatic_aberration = strength.max(0.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_film_grain_strength(&mut self, strength: f32) {
        self.video.film_grain_strength = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    /// Plays the game's own splash effects for footsteps and landings the renderer
    /// found in standing water: `env/water_splash` (droplets and a wake ring) for
    /// a step, `env/water_impact` (the big bodyfall splash) for a landing.
    pub(in crate::app) fn play_water_splashes(
        &mut self,
        splashes: &[crate::weather::wake::WaterSplash],
    ) {
        let Some(session) = self.game_session.as_mut() else {
            return;
        };
        for splash in splashes {
            let mut origin = scene::jka_position(splash.position);
            origin[2] += 1.0;
            let name = if splash.landing {
                "env/water_impact"
            } else {
                "env/water_splash"
            };
            session
                .weapon_fx
                .player_fx(&crate::cgame::player_presenter::PlayerFxRequest::Effect {
                    name,
                    origin,
                    // Forward is the surface normal (up in JKA space).
                    axis: [[0.0, 0.0, 1.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]],
                });
        }
    }

    pub(in crate::app) fn set_puddle_scatter(&mut self, amount: f32) {
        self.video.puddle_scatter = amount.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_rain_grade(&mut self, strength: f32) {
        self.video.rain_grade = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_motion_blur_strength(&mut self, strength: f32) {
        self.video.motion_blur_strength = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_depth_of_field_strength(&mut self, strength: f32) {
        self.video.depth_of_field_strength = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn cycle_dof_quality(&mut self, direction: i32) {
        let current = DofQuality::ALL
            .iter()
            .position(|quality| *quality == self.video.dof_quality)
            .unwrap_or(1) as i32;
        let next = (current + direction).rem_euclid(DofQuality::ALL.len() as i32) as usize;
        self.video.dof_quality = DofQuality::ALL[next];
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_color_lut_strength(&mut self, strength: f32) {
        self.video.color_lut_strength = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_color_lut(&mut self, preset: ColorLutPreset) {
        self.video.color_lut = preset;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_fog_strength(&mut self, strength: f32) {
        self.video.fog_strength = strength.clamp(0.0, ui::MAX_FOG_STRENGTH);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn map_sun_editor_values(&self) -> Option<(f32, f32, f32, [f32; 3])> {
        self.map_authored_sun.map(|sun| {
            let [yaw, pitch] = sun.q3_angles();
            let max_channel = sun.color.into_iter().fold(0.0_f32, f32::max);
            let color = if max_channel > 1e-6 {
                sun.color
                    .map(|channel| (channel / max_channel).clamp(0.0, 1.0))
            } else {
                [1.0, 1.0, 1.0]
            };
            (yaw, pitch, sun.intensity, color)
        })
    }

    pub(in crate::app) fn set_sun_override(&mut self, enabled: bool, seed_from_map: bool) {
        if enabled && !self.video.sun_override && seed_from_map && !self.sun_custom_initialized {
            if let Some((yaw, pitch, intensity, color)) = self.map_sun_editor_values() {
                self.video.sun_yaw = yaw;
                self.video.sun_pitch = pitch;
                self.video.sun_intensity = intensity;
                self.video.sun_color = color;
            }
        }
        if enabled {
            self.sun_custom_initialized = true;
        }
        self.video.sun_override = enabled;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    /// Shows the sun-to-head beam for a moment; called on every yaw/pitch edit and
    /// kept alive by `egui_sun` while a slider is still held.
    pub(in crate::app) fn touch_sun_ray(&mut self) {
        self.sun_ray_until = Some(Instant::now() + std::time::Duration::from_millis(1200));
        self.sync_sun_ray();
    }

    /// Sends the beam's visibility to the renderer when it changes or expires.
    pub(in crate::app) fn sync_sun_ray(&mut self) {
        let want = self
            .sun_ray_until
            .is_some_and(|until| Instant::now() < until);
        if !want {
            self.sun_ray_until = None;
        }
        if want != self.sun_ray_sent {
            self.sun_ray_sent = want;
            self.render_command(RenderCommand::SetSunRayPreview(want));
        }
    }

    pub(in crate::app) fn set_sun_yaw(&mut self, yaw: f32) {
        self.video.sun_yaw = yaw.rem_euclid(360.0);
        self.sun_custom_initialized = true;
        self.touch_sun_ray();
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_sun_pitch(&mut self, pitch: f32) {
        self.video.sun_pitch = pitch.clamp(-90.0, 90.0);
        self.sun_custom_initialized = true;
        self.touch_sun_ray();
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_sun_intensity(&mut self, intensity: f32) {
        self.video.sun_intensity = intensity.max(0.0);
        self.sun_custom_initialized = true;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_sun_color(&mut self, color: [f32; 3]) {
        self.video.sun_color = color.map(|channel| channel.clamp(0.0, 1.0));
        self.sun_custom_initialized = true;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn effective_distance_cull(&self) -> f32 {
        let scale = self.video.distance_cull_scale;
        if scale <= 0.001 {
            self.map_distance_cull
        } else {
            self.map_distance_cull * scale
        }
    }

    pub(in crate::app) fn apply_distance_cull(&mut self) {
        self.camera
            .set_distance_cull(self.effective_distance_cull());
    }

    pub(in crate::app) fn set_distance_cull_scale(&mut self, scale: f32) {
        self.video.distance_cull_scale = scale.clamp(0.0, ui::MAX_DISTANCE_CULL_SCALE);
        self.apply_distance_cull();
        self.mark_config_dirty();
        self.publish_ui();
    }

    /// Exactly `N` finite whitespace-separated numbers, for vector-valued cvars.
    pub(in crate::app) fn parse_cvar_floats<const N: usize>(
        name: &str,
        value: &str,
    ) -> Result<[f32; N], String> {
        let error = || format!("{name}: expected {N} finite numbers");
        let mut out = [0.0_f32; N];
        let mut words = value.split_whitespace();
        for slot in &mut out {
            *slot = words
                .next()
                .and_then(|word| word.parse::<f32>().ok())
                .filter(|number| number.is_finite())
                .ok_or_else(error)?;
        }
        if words.next().is_some() {
            return Err(error());
        }
        Ok(out)
    }

    pub(in crate::app) fn set_cloud_quality(&mut self, value: f32) {
        self.video.cloud_quality = value.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_cloud_coverage(&mut self, value: f32) {
        self.video.cloud_coverage = value.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    /// Commit one CLOUD TUNING row. Every row on that page is either a toggle
    /// or a 0..1 slider, so one entry point covers keyboard, mouse and reset.
    pub(in crate::app) fn apply_cloud_tuning(
        &mut self,
        row: usize,
        toggle: bool,
        slider: Option<f32>,
    ) {
        let set = |value: Option<f32>, current: f32| value.unwrap_or(current).clamp(0.0, 1.0);
        match row {
            ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX => {
                if toggle {
                    self.video.cloud_temporal_depth_fix = !self.video.cloud_temporal_depth_fix;
                }
            }
            ui::CLOUD_ROW_SHEAR => {
                self.video.cloud_shear = set(slider, self.video.cloud_shear);
            }
            ui::CLOUD_ROW_BASE_VARIATION => {
                self.video.cloud_base_variation = set(slider, self.video.cloud_base_variation);
            }
            ui::CLOUD_ROW_SHAPE_EVOLUTION => {
                if toggle {
                    self.video.cloud_shape_evolution = !self.video.cloud_shape_evolution;
                }
            }
            ui::CLOUD_ROW_TERRAIN_INTERACTION => {
                if toggle {
                    self.video.cloud_terrain_interaction = !self.video.cloud_terrain_interaction;
                }
            }
            ui::CLOUD_ROW_EMPTY_SKIP => {
                if toggle {
                    self.video.cloud_empty_skip = !self.video.cloud_empty_skip;
                }
            }
            ui::CLOUD_ROW_AERIAL => {
                self.video.cloud_aerial = set(slider, self.video.cloud_aerial);
            }
            ui::CLOUD_ROW_SKY_AMBIENT => {
                if toggle {
                    self.video.cloud_sky_ambient = !self.video.cloud_sky_ambient;
                }
            }
            ui::CLOUD_ROW_HISTORY_BLEND => {
                self.video.cloud_history_blend = set(slider, self.video.cloud_history_blend);
            }
            ui::CLOUD_ROW_MOTION_REJECT => {
                self.video.cloud_motion_reject = set(slider, self.video.cloud_motion_reject);
            }
            ui::CLOUD_ROW_THICKNESS_VARIATION => {
                self.video.cloud_thickness_variation =
                    set(slider, self.video.cloud_thickness_variation);
            }
            ui::CLOUD_ROW_SIZE => {
                self.video.cloud_size = set(slider, self.video.cloud_size);
            }
            _ => {
                if toggle {
                    self.video.cloud_history_depth_reject = !self.video.cloud_history_depth_reject;
                }
            }
        }
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_cloud_height(&mut self, value: f32) {
        self.video.cloud_height = value.clamp(ui::CLOUD_HEIGHT_MIN, ui::CLOUD_HEIGHT_MAX);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_cloud_thickness(&mut self, value: f32) {
        self.video.cloud_thickness = value.clamp(ui::CLOUD_THICKNESS_MIN, ui::CLOUD_THICKNESS_MAX);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_weather_wind(&mut self, wind: crate::ocean::OceanWind) {
        let wind = wind.sanitize();
        self.video.weather_wind = wind;
        // OceanSettings retains the simulation-facing copy, but Weather is the
        // only authored/UI source. Keep every ocean instance on the same wind.
        self.video.ocean_settings.wind = wind;
        self.sync_post_effects();
        self.render_command(RenderCommand::SetOceanSettings(self.video.ocean_settings));
        self.publish_authored_oceans();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn set_weather_wind_speed(&mut self, value: f32) {
        let mut wind = self.video.weather_wind;
        wind.speed = value;
        self.set_weather_wind(wind);
    }

    pub(in crate::app) fn set_weather_wind_direction(&mut self, value: f32) {
        let mut wind = self.video.weather_wind;
        wind.direction = value.rem_euclid(360.0);
        self.set_weather_wind(wind);
    }

    pub(in crate::app) fn set_weather_gust_strength(&mut self, value: f32) {
        let mut wind = self.video.weather_wind;
        wind.gust = value;
        self.set_weather_wind(wind);
    }

    pub(in crate::app) fn set_weather_direction_variation(&mut self, value: f32) {
        let mut wind = self.video.weather_wind;
        wind.shift = value;
        self.set_weather_wind(wind);
    }

    pub(in crate::app) fn sync_post_effects(&self) {
        self.render_command(RenderCommand::SetPostEffects(PostEffects {
            hdr: self.video.hdr,
            auto_exposure: self.video.auto_exposure,
            tone_mapping: self.video.tone_mapping,
            bloom: self.video.bloom,
            halation: self.video.halation,
            ssao: self.video.ssao,
            static_bsp_ao: self.video.static_bsp_ao,
            static_bsp_ao_lightmap: self.video.static_bsp_ao_lightmap,
            static_bsp_ao_samples: self.video.static_bsp_ao_samples,
            static_bsp_ao_resolution: self.video.static_bsp_ao_resolution,
            static_bsp_ao_strength: self.video.static_bsp_ao_strength,
            static_bsp_ao_range: self.video.static_bsp_ao_range,
            static_bsp_ao_current_cell: self.video.static_bsp_ao_current_cell,
            fxaa: self.video.fxaa,
            smaa: self.video.smaa,
            taa: self.video.taa,
            contact_shadows: self.video.contact_shadows,
            fog_mode: self.video.fog_mode,
            fog_strength: self.video.fog_strength,
            sun_override: self.video.sun_override,
            sun_yaw: self.video.sun_yaw,
            sun_pitch: self.video.sun_pitch,
            sun_intensity: self.video.sun_intensity,
            sun_color: self.video.sun_color,
            sun_visibility: self.video.sun_visibility,
            entity_sun_lighting: self.video.entity_sun_lighting,
            clouds: self.video.clouds,
            cloud_type: self.video.cloud_type,
            cloud_quality: self.video.cloud_quality,
            cloud_coverage: self.video.cloud_coverage,
            cloud_height: self.video.cloud_height,
            cloud_thickness: self.video.cloud_thickness,
            weather_wind: self.video.weather_wind,
            cloud_shadows: self.video.cloud_shadows,
            cloud_render_resolution: self.video.cloud_render_resolution,
            cloud_temporal: self.video.cloud_temporal,
            cloud_temporal_depth_fix: self.video.cloud_temporal_depth_fix,
            cloud_shear: self.video.cloud_shear,
            cloud_base_variation: self.video.cloud_base_variation,
            cloud_shape_evolution: self.video.cloud_shape_evolution,
            cloud_terrain_interaction: self.video.cloud_terrain_interaction,
            cloud_empty_skip: self.video.cloud_empty_skip,
            cloud_aerial: self.video.cloud_aerial,
            cloud_sky_ambient: self.video.cloud_sky_ambient,
            cloud_history_blend: self.video.cloud_history_blend,
            cloud_motion_reject: self.video.cloud_motion_reject,
            cloud_history_depth_reject: self.video.cloud_history_depth_reject,
            cloud_thickness_variation: self.video.cloud_thickness_variation,
            cloud_size: self.video.cloud_size,
            rain: self.video.rain,
            rain_intensity: self.video.rain_intensity,
            puddle_quality: self.video.puddle_quality,
            puddle_scatter: self.video.puddle_scatter,
            rain_grade: self.video.rain_grade,
            // Planar eligibility changes BSP batching/topology, so the renderer
            // runs the best quality the loaded map was prepared to serve.
            reflection_quality: self.effective_reflection_quality(),
            reflection_debug: self.video.reflection_debug,
            chromatic_aberration: self.video.chromatic_aberration,
            vignette: self.video.vignette,
            film_grain_strength: self.video.film_grain_strength,
            motion_blur_strength: self.video.motion_blur_strength,
            depth_of_field_strength: self.video.depth_of_field_strength,
            dof_quality: self.video.dof_quality,
            color_lut: self.video.color_lut,
            color_lut_strength: self.video.color_lut_strength,
            split_toning: self.video.split_toning,
        }));
    }

    pub(in crate::app) fn sync_gpu_visibility(&self) {
        self.render_command(RenderCommand::SetGpuVisibility {
            gpu_driven: self.video.gpu_driven,
            hiz_occlusion: self.video.hiz_occlusion,
        });
    }

    pub(in crate::app) fn sync_dynamic_lighting(&self) {
        self.render_command(RenderCommand::SetDynamicLighting(self.video.dynamic_lights));
        self.render_command(RenderCommand::SetRtSamples(self.video.rt_samples));
        self.render_command(RenderCommand::SetDynamicLightFalloff(
            self.video.dynamic_light_falloff,
        ));
        self.render_command(RenderCommand::SetRtHalfResolution(
            self.video.rt_half_resolution,
        ));
    }

    pub(in crate::app) fn sync_classic_world_lighting(&self) {
        self.render_command(RenderCommand::SetClassicWorldLighting {
            fullbright: !self.video.world_lighting,
            vertex_light: self.video.vertex_lighting,
            lightmap_only: self.video.lightmap_only,
        });
    }

    pub(in crate::app) fn sync_map_light_simulation(&self) {
        self.render_command(RenderCommand::SetMapLightSimulation(
            self.video.map_light_simulation,
        ));
    }

    pub(in crate::app) fn sync_entity_ambient_lighting(&self) {
        self.render_command(RenderCommand::SetEntityAmbientLighting(
            self.video.entity_ambient_lighting,
        ));
    }

    pub(in crate::app) fn sync_emissive_area_lights(&self) {
        self.render_command(RenderCommand::SetEmissiveAreaLights(
            self.video.emissive_area_lights,
        ));
    }

    pub(in crate::app) fn sync_voxel_probe_gi(&self) {
        self.render_command(RenderCommand::SetVoxelProbeGi(self.video.voxel_probe_gi));
    }

    pub(in crate::app) fn sync_local_light_shadows(&self) {
        self.render_command(RenderCommand::SetLocalLightShadows(
            self.video.local_light_shadows,
        ));
    }

    pub(in crate::app) fn sync_pbr(&self) {
        self.render_command(RenderCommand::SetPbrSettings {
            enabled: self.video.pbr,
            deluxe_mapping: self.video.deluxe_mapping,
            deluxe_specular: self.video.deluxe_specular,
        });
    }

    pub(in crate::app) fn sync_entity_shadow_light(&self) {
        self.render_command(RenderCommand::SetEntityShadowLight(
            self.video.entity_shadow_light,
        ));
    }

    pub(in crate::app) fn sync_cascaded_shadows(&self) {
        self.render_command(RenderCommand::SetCascadedShadows(
            self.video.dynamic_shadows,
        ));
    }

    pub(in crate::app) fn sync_cull_debug(&self) {
        self.render_command(RenderCommand::SetCullDebug(self.video.cull_debug));
    }

    pub(in crate::app) fn sync_planar_reflection_debug(&self) {
        self.render_command(RenderCommand::SetPlanarReflectionDebug(
            self.video.planar_reflection_debug,
        ));
    }

    pub(in crate::app) fn change_video_setting(&mut self, direction: i32) {
        match self.video_selected {
            ui::VIDEO_ROW_FULLSCREEN => self.cycle_fullscreen(direction),
            ui::VIDEO_ROW_RESOLUTION => self.cycle_resolution(direction),
            ui::VIDEO_ROW_VSYNC => {
                let index = VsyncMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.vsync)
                    .unwrap_or(0);
                let next = (index as i32 + direction).rem_euclid(VsyncMode::ALL.len() as i32);
                self.video.vsync = VsyncMode::ALL[next as usize];
                self.render_command(RenderCommand::SetVsync(self.video.vsync));
                self.mark_config_dirty();
                // Entering FAST on DX12 can make the standing cap unreachable.
                self.set_fps_cap(self.video.fps_cap);
            }
            ui::VIDEO_ROW_MAX_FRAME_LATENCY => {
                let next = (self.video.max_frame_latency as i32 + direction - 1).rem_euclid(3) + 1;
                self.set_max_frame_latency(next as u32);
            }
            ui::VIDEO_ROW_FPS_CAP => self.begin_fps_cap_edit(),
            ui::VIDEO_ROW_PHYSICS_FPS => self.begin_physics_fps_edit(),
            ui::VIDEO_ROW_BRIGHTNESS => self.set_gamma(self.video.gamma + direction as f32 * 0.05),
            ui::VIDEO_ROW_ANTI_ALIASING => self.cycle_anti_aliasing(direction),
            ui::VIDEO_ROW_TEXTURE_FILTER => self.cycle_texture_filter(direction),
            ui::VIDEO_ROW_PICMIP => {
                // Menu quality runs low -> high; r_picmip runs high -> low.
                const VALUES: [u32; 5] = [4, 3, 2, 1, 0];
                let current = VALUES
                    .iter()
                    .position(|value| *value == self.video.picmip.min(4))
                    .unwrap_or(0) as i32;
                let next = (current + direction).rem_euclid(VALUES.len() as i32) as usize;
                self.video.picmip = VALUES[next];
                self.render_command(RenderCommand::SetPicmip(self.video.picmip));
                self.console_status = format!(
                    "TEXTURE QUALITY: r_picmip {} - APPLY VIDEO SETTINGS TO RELOAD THE CURRENT MAP",
                    self.video.picmip
                );
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DETAIL_TEXTURES => {
                let current = DetailTextureMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.detail_textures)
                    .unwrap_or(0) as i32;
                let next = (current + direction).rem_euclid(DetailTextureMode::ALL.len() as i32);
                self.video.detail_textures = DetailTextureMode::ALL[next as usize];
                self.render_command(RenderCommand::SetDetailTextures(self.video.detail_textures));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PVS => {
                const MODES: [PvsMode; 4] =
                    [PvsMode::Off, PvsMode::Minimal, PvsMode::Full, PvsMode::Auto];
                let current = MODES
                    .iter()
                    .position(|mode| *mode == self.video.pvs_mode)
                    .unwrap_or(3) as i32;
                let next = (current + direction).rem_euclid(MODES.len() as i32) as usize;
                self.video.pvs_mode = MODES[next];
                self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DISTANCE_CULL => {
                self.set_distance_cull_scale(
                    self.video.distance_cull_scale + direction as f32 * 0.5,
                );
            }
            ui::VIDEO_ROW_GPU_DRIVEN => {
                self.video.gpu_driven = !self.video.gpu_driven;
                self.sync_gpu_visibility();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_HIZ => {
                self.video.hiz_occlusion = !self.video.hiz_occlusion;
                self.sync_gpu_visibility();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING => {
                let current = EntityAmbientLightingMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.entity_ambient_lighting)
                    .unwrap_or(0) as i32;
                let next = (current + direction)
                    .rem_euclid(EntityAmbientLightingMode::ALL.len() as i32)
                    as usize;
                self.video.entity_ambient_lighting = EntityAmbientLightingMode::ALL[next];
                self.sync_entity_ambient_lighting();
                self.mark_config_dirty();
                self.console_status = format!(
                    "ENTITY AMBIENT LIGHTING: {}",
                    self.video.entity_ambient_lighting.label()
                );
            }
            ui::VIDEO_ROW_PBR => {
                self.video.pbr = !self.video.pbr;
                self.mark_config_dirty();
                self.console_status = if self.map_prepare_restart_required() {
                    format!(
                        "PHYSICALLY BASED RENDERING (PBR): {} - MATERIAL SOURCE CHANGES ON APPLY / VID_RESTART",
                        if self.video.pbr { "ON" } else { "OFF" }
                    )
                } else {
                    format!(
                        "PHYSICALLY BASED RENDERING (PBR): {}",
                        if self.video.pbr { "ON" } else { "OFF" }
                    )
                };
            }
            ui::VIDEO_ROW_ASSET_OVERRIDES => {
                self.video.allow_asset_overrides = !self.video.allow_asset_overrides;
                self.mark_config_dirty();
                self.console_status = format!(
                    "ALLOW ASSET OVERRIDES: {}{}",
                    if self.video.allow_asset_overrides {
                        "ON"
                    } else {
                        "OFF - RETAIL ASSETS PROTECTED"
                    },
                    if self.map_prepare_restart_required() {
                        " - APPLY VIDEO SETTINGS OR RUN VID_RESTART"
                    } else {
                        ""
                    }
                );
            }
            ui::VIDEO_ROW_GEN_NORMAL_MAPS => {
                self.video.gen_normal_maps = !self.video.gen_normal_maps;
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "GENERATED NORMAL MAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART"
                            .into();
                }
            }
            ui::VIDEO_ROW_DELUXE_MAPPING => {
                self.video.deluxe_mapping = !self.video.deluxe_mapping;
                self.sync_pbr();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DELUXE_SPECULAR => {
                self.video.deluxe_specular =
                    (self.video.deluxe_specular + direction as f32 * 0.1).clamp(0.0, 1.0);
                self.sync_pbr();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_WORLD_LIGHTING | ui::VIDEO_ROW_VERTEX_LIGHTING => {
                let current = if !self.video.world_lighting {
                    0
                } else if self.video.vertex_lighting {
                    1
                } else {
                    2
                };
                let next = (current as i32 + direction).rem_euclid(3) as usize;
                self.set_world_lighting_quality(next);
            }
            ui::VIDEO_ROW_LIGHTMAP_ONLY => {
                self.video.lightmap_only = !self.video.lightmap_only;
                self.sync_classic_world_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DYNAMIC_LIGHTS => {
                let current = DynamicLightsMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.dynamic_lights)
                    .unwrap_or(0) as i32;
                let next =
                    (current + direction).rem_euclid(DynamicLightsMode::ALL.len() as i32) as usize;
                self.video.dynamic_lights = DynamicLightsMode::ALL[next];
                self.sync_dynamic_lighting();
                self.mark_config_dirty();
                if matches!(
                    self.video.dynamic_lights,
                    DynamicLightsMode::RayTracedHardware
                ) {
                    self.console_status = "DYNAMIC LIGHTS: HARDWARE RT DIRECT LIGHTING (FORWARD+ FALLBACK IF UNAVAILABLE)".into();
                }
            }
            ui::VIDEO_ROW_MAP_LIGHT_SIMULATION => {
                self.video.map_light_simulation = !self.video.map_light_simulation;
                self.sync_map_light_simulation();
                self.mark_config_dirty();
                self.console_status = format!(
                    ".MAP LIGHT SIMULATION: {}",
                    if self.video.map_light_simulation {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            ui::VIDEO_ROW_MODERN_SABERS => {
                self.video.modern_sabers = !self.video.modern_sabers;
                if let Some(session) = self.game_session.as_mut() {
                    session
                        .weapon_fx
                        .set_modern_sabers(self.video.modern_sabers);
                }
                self.mark_config_dirty();
                self.console_status = format!(
                    "MODERN SABER RENDERING: {}",
                    if self.video.modern_sabers {
                        "ON"
                    } else {
                        "OFF (OPENJK)"
                    }
                );
            }
            ui::VIDEO_ROW_FLARES => {
                self.video.flares = !self.video.flares;
                self.mark_config_dirty();
                self.console_status =
                    format!("FLARES: {}", if self.video.flares { "ON" } else { "OFF" });
            }
            ui::VIDEO_ROW_SABER_IMPACT_FX => {
                self.video.saber_impact_fx = !self.video.saber_impact_fx;
                if let Some(session) = self.game_session.as_mut() {
                    session
                        .weapon_fx
                        .set_saber_impact_fx(self.video.saber_impact_fx);
                }
                self.mark_config_dirty();
                self.console_status = format!(
                    "SABER IMPACT EFFECTS: {}",
                    if self.video.saber_impact_fx {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            ui::VIDEO_ROW_SABER_MARKS => {
                let current = ui::SaberMarkMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.saber_marks)
                    .unwrap_or(1) as i32;
                let next =
                    (current + direction).rem_euclid(ui::SaberMarkMode::ALL.len() as i32) as usize;
                self.video.saber_marks = ui::SaberMarkMode::ALL[next];
                if let Some(session) = self.game_session.as_mut() {
                    session.weapon_fx.set_saber_marks(self.video.saber_marks);
                }
                self.mark_config_dirty();
                self.console_status = format!(
                    "SABER MARKS: {}",
                    self.video.saber_marks.label().to_ascii_uppercase()
                );
            }
            ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS => {
                self.video.emissive_area_lights = !self.video.emissive_area_lights;
                self.sync_emissive_area_lights();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_AMBIENT_OCCLUSION => {
                // Final user-facing AO selector: OFF / REALTIME / BAKED.
                // BAKED always uses the hybrid HQ path at the fixed quality target.
                let mode = if self.video.ssao {
                    1_i32
                } else if self.video.static_bsp_ao {
                    2
                } else {
                    0
                };
                match (mode + direction).rem_euclid(3) {
                    0 => {
                        self.video.ssao = false;
                        self.video.static_bsp_ao = false;
                    }
                    1 => {
                        self.video.ssao = true;
                        self.video.static_bsp_ao = false;
                    }
                    _ => {
                        self.video.ssao = false;
                        self.video.static_bsp_ao = true;
                        self.video.static_bsp_ao_lightmap = true;
                    }
                }
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_VOXEL_PROBE_GI => {
                self.video.voxel_probe_gi = !self.video.voxel_probe_gi;
                self.sync_voxel_probe_gi();
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "VOXEL / PROBE GI: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
            }
            ui::VIDEO_ROW_DYNAMIC_SHADOWS => {
                let current = DynamicShadowsMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.dynamic_shadows)
                    .unwrap_or(0) as i32;
                let next =
                    (current + direction).rem_euclid(DynamicShadowsMode::ALL.len() as i32) as usize;
                self.video.dynamic_shadows = DynamicShadowsMode::ALL[next];
                self.video.cascaded_shadows =
                    self.video.dynamic_shadows == DynamicShadowsMode::CascadedShadowMaps;
                self.sync_cascaded_shadows();
                self.mark_config_dirty();
                if self.video.dynamic_shadows == DynamicShadowsMode::Blob {
                    self.console_status =
                        "DYNAMIC SHADOWS: BLOB - OPENJK CG_SHADOWS 1 DROP SHADOWS".to_string();
                } else if self.video.dynamic_shadows == DynamicShadowsMode::EntityMap {
                    self.console_status =
                        "DYNAMIC SHADOWS: ENTITY MAP - PLAYERS/NPCS CAST FROM THE BAKED LIGHTGRID DIRECTION".to_string();
                } else if self.video.dynamic_shadows == DynamicShadowsMode::RayTraced {
                    self.console_status =
                        "DYNAMIC SHADOWS: RAY TRACED SHADOWS - HARDWARE RAY QUERY (STATIC OPAQUE BSP CASTERS)".to_string();
                }
            }
            ui::VIDEO_ROW_ENTITY_SHADOW_LIGHT => {
                let all = EntityShadowLight::ALL;
                let current = all
                    .iter()
                    .position(|source| *source == self.video.entity_shadow_light)
                    .unwrap_or(0) as i32;
                self.video.entity_shadow_light =
                    all[(current + direction).rem_euclid(all.len() as i32) as usize];
                self.sync_entity_shadow_light();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS => {
                self.video.local_light_shadows = !self.video.local_light_shadows;
                self.sync_local_light_shadows();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CONTACT_SHADOWS => {
                self.video.contact_shadows = !self.video.contact_shadows;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_SSR => {
                let current = ReflectionQuality::ALL
                    .iter()
                    .position(|quality| *quality == self.video.reflection_quality)
                    .unwrap_or(4) as i32;
                let next = (current + direction).clamp(0, ReflectionQuality::ALL.len() as i32 - 1)
                    as usize;
                self.video.reflection_quality = ReflectionQuality::ALL[next];
                self.reflection_quality_changed();
            }
            ui::VIDEO_ROW_PLANAR_REFLECTIONS => {
                self.video.reflection_debug = !self.video.reflection_debug;
                self.sync_post_effects();
            }
            ui::VIDEO_ROW_PLANAR_REFLECTION_DEBUG => {
                let current = PlanarReflectionDebugMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.planar_reflection_debug)
                    .unwrap_or(0) as i32;
                let next = (current + direction)
                    .rem_euclid(PlanarReflectionDebugMode::ALL.len() as i32)
                    as usize;
                self.video.planar_reflection_debug = PlanarReflectionDebugMode::ALL[next];
                self.sync_planar_reflection_debug();
            }
            ui::VIDEO_ROW_HDR => {
                self.video.hdr = !self.video.hdr;
                self.sync_post_effects();
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "HDR / FLOAT LIGHTMAP FORMAT: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
            }
            ui::VIDEO_ROW_FLOAT_LIGHTMAP => {
                self.video.float_lightmap = !self.video.float_lightmap;
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "FLOAT LIGHTMAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
            }
            ui::VIDEO_ROW_TONE_MAPPING => {
                self.video.tone_mapping = !self.video.tone_mapping;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_AUTO_EXPOSURE => {
                self.video.auto_exposure = !self.video.auto_exposure;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BLOOM => {
                self.video.bloom = !self.video.bloom;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_MOTION_BLUR => self.set_motion_blur_strength(
                self.video.motion_blur_strength + direction as f32 * 0.05,
            ),
            ui::VIDEO_ROW_DEPTH_OF_FIELD => self.set_depth_of_field_strength(
                self.video.depth_of_field_strength + direction as f32 * 0.05,
            ),
            ui::VIDEO_ROW_DOF_QUALITY => self.cycle_dof_quality(direction),
            ui::VIDEO_ROW_HALATION => {
                self.video.halation = !self.video.halation;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CHROMATIC_ABERRATION => {
                let step = if self.video.chromatic_aberration < 1.0 {
                    0.05
                } else if self.video.chromatic_aberration < 10.0 {
                    0.5
                } else {
                    5.0
                };
                self.set_chromatic_aberration_strength(
                    self.video.chromatic_aberration + direction as f32 * step,
                );
            }
            ui::VIDEO_ROW_VIGNETTE => {
                self.video.vignette = !self.video.vignette;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FILM_GRAIN => self
                .set_film_grain_strength(self.video.film_grain_strength + direction as f32 * 0.05),
            ui::VIDEO_ROW_COLOR_LUT => {
                let all = ColorLutPreset::all();
                let current = all
                    .iter()
                    .position(|preset| *preset == self.video.color_lut)
                    .unwrap_or(0) as i32;
                let next = (current + direction).rem_euclid(all.len() as i32) as usize;
                self.set_color_lut(all[next]);
            }
            ui::VIDEO_ROW_LUT_STRENGTH => {
                self.set_color_lut_strength(self.video.color_lut_strength + direction as f32 * 0.05)
            }
            ui::VIDEO_ROW_WIREFRAME => {
                if self.wireframe_supported {
                    self.video.wireframe_mask = if self.video.wireframe_mask == 0 {
                        ui::wireframe::MAP
                    } else {
                        0
                    };
                    self.render_command(RenderCommand::SetWireframeMask(self.video.wireframe_mask));
                    self.mark_config_dirty();
                } else {
                    self.console_status = "WIREFRAME OVERLAY IS NOT SUPPORTED BY THIS GPU".into();
                }
            }
            ui::VIDEO_ROW_CULL_DEBUG => {
                self.video.cull_debug = match self.video.cull_debug {
                    CullDebugMode::Off => CullDebugMode::RejectionReasons,
                    CullDebugMode::RejectionReasons => CullDebugMode::Off,
                };
                self.sync_cull_debug();
            }
            ui::VIDEO_ROW_DRAW_FPS => {
                self.video.draw_fps =
                    ((self.video.draw_fps as i32 + direction).rem_euclid(3)) as u8;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DEVELOPER_TOOLS => {
                self.video.developer_level =
                    ((i32::from(self.video.developer_level) + direction).rem_euclid(4)) as u8;
                self.video.developer_tools = self.video.developer_level != 0;
                crate::logging::set_developer_level(self.video.developer_level);
                if !self.video.developer_tools {
                    self.forget_trace();
                    self.render_command(RenderCommand::ClearSurfaceInspection);
                }
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_RENDERER_VERBOSE => {
                self.video.renderer_verbose =
                    ((i32::from(self.video.renderer_verbose) + direction).rem_euclid(4)) as u8;
                crate::logging::set_renderer_verbose_level(self.video.renderer_verbose);
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PERF_TRACE => {
                self.video.perf_trace = !self.video.perf_trace;
                self.render_command(RenderCommand::SetPerfTrace(self.video.perf_trace));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GPU_TIMINGS => {
                self.video.gpu_timings = !self.video.gpu_timings;
                self.render_command(RenderCommand::SetGpuTimings(self.video.gpu_timings));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FX_GEOMETRY => {
                let current = FxGeometryMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.fx_geometry)
                    .unwrap_or(0) as i32;
                let next =
                    (current + direction).rem_euclid(FxGeometryMode::ALL.len() as i32) as usize;
                self.video.fx_geometry = FxGeometryMode::ALL[next];
                self.mark_config_dirty();
                self.console_status = format!("FX GEOMETRY: {}", self.video.fx_geometry.label());
            }
            ui::VIDEO_ROW_FX_PHYSICS => {
                const MODES: [u32; 3] = [
                    crate::fx::FX_PHYSICS_OFF,
                    crate::fx::FX_PHYSICS_AUTHORED,
                    crate::fx::FX_PHYSICS_ALL,
                ];
                let current = MODES
                    .iter()
                    .position(|mode| *mode == self.video.fx_physics)
                    .unwrap_or(1) as i32;
                self.video.fx_physics =
                    MODES[(current + direction).rem_euclid(MODES.len() as i32) as usize];
                self.apply_fx_physics();
                self.mark_config_dirty();
                self.console_status = format!(
                    "FX PHYSICS: {}",
                    crate::fx::physics_label(self.video.fx_physics)
                );
            }
            ui::VIDEO_ROW_FX_LOD => {
                const MODES: [u32; 3] = [
                    crate::fx::FX_LOD_OFF,
                    crate::fx::FX_LOD_AUTHORED,
                    crate::fx::FX_LOD_ADAPTIVE,
                ];
                let current = MODES
                    .iter()
                    .position(|mode| *mode == self.video.fx_lod)
                    .unwrap_or(2) as i32;
                self.video.fx_lod =
                    MODES[(current + direction).rem_euclid(MODES.len() as i32) as usize];
                self.apply_fx_lod();
                self.mark_config_dirty();
                self.console_status =
                    format!("FX LOD: {}", crate::fx::lod_label(self.video.fx_lod));
            }
            ui::VIDEO_ROW_DRAW_TRIGGERS => {
                self.video.draw_triggers = !self.video.draw_triggers;
                self.sync_debug_volumes();
                self.console_status = format!(
                    "DRAW TRIGGERS: {}",
                    if self.video.draw_triggers {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            ui::VIDEO_ROW_DRAW_CLIP_BRUSHES => {
                self.video.draw_clip_brushes = !self.video.draw_clip_brushes;
                self.sync_debug_volumes();
                self.console_status = format!(
                    "DRAW CLIP BRUSHES: {}",
                    if self.video.draw_clip_brushes {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            ui::VIDEO_ROW_DRAW_ENTITIES => {
                self.set_draw_entities(!self.video.draw_entities);
                self.console_status = format!(
                    "DRAW ENTITIES: {}",
                    if self.video.draw_entities {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            ui::VIDEO_ROW_DRAW_MAP_MODELS => {
                self.video.draw_map_models = !self.video.draw_map_models;
                self.mark_config_dirty();
                self.console_status = format!(
                    "MAP MODELS: {}",
                    if self.video.draw_map_models {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            ui::VIDEO_ROW_FX_ZERO_ALPHA_DISCARD => {
                self.video.fx_zero_alpha_discard = !self.video.fx_zero_alpha_discard;
                self.render_command(RenderCommand::SetFxZeroAlphaDiscard(
                    self.video.fx_zero_alpha_discard,
                ));
                self.mark_config_dirty();
                self.console_status = format!(
                    "FX ZERO-ALPHA DISCARD: {}",
                    if self.video.fx_zero_alpha_discard {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            ui::VIDEO_ROW_GHOUL2_SKINNING => {
                let current = Ghoul2SkinningMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.ghoul2_skinning)
                    .unwrap_or(0) as i32;
                let next =
                    (current + direction).rem_euclid(Ghoul2SkinningMode::ALL.len() as i32) as usize;
                self.video.ghoul2_skinning = Ghoul2SkinningMode::ALL[next];
                self.mark_config_dirty();
                self.console_status =
                    format!("GHOUL2 SKINNING: {}", self.video.ghoul2_skinning.label());
            }
            ui::VIDEO_ROW_GHOUL2_LOD_BIAS => {
                self.video.ghoul2_lod_bias = (self.video.ghoul2_lod_bias + direction).clamp(0, 8);
                self.mark_config_dirty();
                self.console_status = format!("MODEL LOD BIAS: {}", self.video.ghoul2_lod_bias);
            }
            ui::VIDEO_ROW_GHOUL2_BATCH_DRAWS => {
                let current = Ghoul2BatchMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.ghoul2_batch_draws)
                    .unwrap_or(1) as i32;
                let next =
                    (current + direction).rem_euclid(Ghoul2BatchMode::ALL.len() as i32) as usize;
                self.video.ghoul2_batch_draws = Ghoul2BatchMode::ALL[next];
                self.render_command(RenderCommand::SetGhoul2BatchDraws(
                    self.video.ghoul2_batch_draws,
                ));
                self.mark_config_dirty();
                self.console_status = format!(
                    "GHOUL2 BATCH DRAWS: {}",
                    self.video.ghoul2_batch_draws.label()
                );
            }
            ui::VIDEO_ROW_GHOUL2_EARLY_CULL => {
                self.video.ghoul2_early_cull = !self.video.ghoul2_early_cull;
                self.mark_config_dirty();
                self.console_status = format!(
                    "GHOUL2 EARLY CULL: {}",
                    if self.video.ghoul2_early_cull {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            ui::VIDEO_ROW_RENDER_BACKEND => self.cycle_renderer_backend(direction),
            ui::VIDEO_ROW_BAKED_AO_SAMPLES => {
                const VALUES: [u32; 5] = [8, 16, 32, 64, 128];
                let current = VALUES
                    .iter()
                    .position(|&v| v == self.video.static_bsp_ao_samples)
                    .unwrap_or(2) as i32;
                self.video.static_bsp_ao_samples =
                    VALUES[(current + direction).rem_euclid(VALUES.len() as i32) as usize];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_RESOLUTION => {
                // Odd scales keep original lightmap texel centers on the refined lattice,
                // so AO=1 cannot brighten/smooth the authored lightmap like 2x/4x did.
                const VALUES: [u32; 3] = [1, 3, 5];
                let current = VALUES
                    .iter()
                    .position(|&v| v == self.video.static_bsp_ao_resolution)
                    .unwrap_or(1) as i32;
                self.video.static_bsp_ao_resolution =
                    VALUES[(current + direction).rem_euclid(VALUES.len() as i32) as usize];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_STRENGTH => {
                const VALUES: [u32; 4] = [25, 50, 75, 100];
                let current = VALUES
                    .iter()
                    .position(|&v| v == self.video.static_bsp_ao_strength)
                    .unwrap_or(2) as i32;
                self.video.static_bsp_ao_strength =
                    VALUES[(current + direction).rem_euclid(VALUES.len() as i32) as usize];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_RANGE => {
                const VALUES: [u32; 4] = [50, 100, 150, 200];
                let current = VALUES
                    .iter()
                    .position(|&v| v == self.video.static_bsp_ao_range)
                    .unwrap_or(1) as i32;
                self.video.static_bsp_ao_range =
                    VALUES[(current + direction).rem_euclid(VALUES.len() as i32) as usize];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_CURRENT_CELL => {
                self.video.static_bsp_ao_current_cell = !self.video.static_bsp_ao_current_cell;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_VID_RESTART if self.video_restart_required() => {
                self.apply_video_settings();
                return;
            }
            ui::VIDEO_ROW_VID_RESTART => {}
            _ => {}
        }
        self.publish_ui();
    }

    pub(in crate::app) fn set_environment_quality_segment(&mut self, row: usize, target: usize) {
        self.environment_selected = row;
        match row {
            ui::ENV_ROW_CLOUD_RENDER_RESOLUTION => {
                const VALUES: [CloudRenderResolution; 4] = [
                    CloudRenderResolution::Quarter,
                    CloudRenderResolution::Half,
                    CloudRenderResolution::ThreeQuarter,
                    CloudRenderResolution::Full,
                ];
                let Some(value) = VALUES.get(target).copied() else {
                    return;
                };
                if self.video.cloud_render_resolution != value {
                    self.video.cloud_render_resolution = value;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                }
            }
            ui::ENV_ROW_FOOTPRINTS => {
                let Some(value) = FootprintMode::ALL.get(target).copied() else {
                    return;
                };
                if self.video.footprints != value {
                    self.video.footprints = value;
                    self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                    self.footprints_chosen();
                    self.mark_config_dirty();
                }
            }
            _ => return,
        }
        self.publish_ui();
    }

    pub(in crate::app) fn commit_ocean_settings(&mut self) {
        self.video.ocean_settings.wind = self.video.weather_wind;
        self.video.ocean_settings = self.video.ocean_settings.sanitize();
        self.render_command(RenderCommand::SetOceanSettings(self.video.ocean_settings));
        self.publish_authored_oceans();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app) fn publish_authored_oceans(&mut self) {
        let mut oceans = self.authored_oceans.clone();
        if self.authored_ocean_preview {
            if let Some(ocean) = oceans.get_mut(self.authored_ocean_selected) {
                ocean.waves = self.video.ocean_settings.authored;
            }
        }
        // There is one atmospheric wind for the whole rendered world. Map/server
        // ocean descriptors still author swell and volume data, but do not create
        // a second independent wind source.
        for ocean in &mut oceans {
            ocean.wind = self.video.weather_wind;
        }
        self.render_command(RenderCommand::SetAuthoredOceans(oceans));
    }

    pub(in crate::app) fn refresh_authored_oceans(&mut self) {
        use crate::ocean::authoring::{AuthoredOcean, CS_OCEANS, CS_WEATHER};
        let mut oceans = if self.game_session.is_none() {
            self.map_authored_oceans.clone()
        } else {
            Vec::new()
        };
        if let Some(session) = &self.game_session {
            for index in 0..8 {
                let Some(bytes) = session.client_game.configstring(CS_OCEANS + index as u16) else {
                    continue;
                };
                if let Some(mut ocean) = AuthoredOcean::parse(index, bytes) {
                    if let Some(weather) = session
                        .client_game
                        .configstring(CS_WEATHER + ocean.weather as u16)
                    {
                        ocean.apply_weather(weather);
                    }
                    oceans.push(ocean);
                }
            }
        }
        if oceans != self.authored_oceans {
            self.authored_oceans = oceans;
            self.authored_ocean_selected = self
                .authored_ocean_selected
                .min(self.authored_oceans.len().saturating_sub(1));
            if self.authored_oceans.is_empty() {
                self.authored_ocean_preview = false;
            }
            self.publish_authored_oceans();
        }
        if let Some(net) = &self.net {
            self.render_command(RenderCommand::SetOceanTime(
                net.session().server_time() as f32 * 0.001,
            ));
        } else if let Some(time) = self
            .game_session
            .as_ref()
            .and_then(|s| s.timeline.as_ref())
            .map(|t| t.target_time_ms(Instant::now()))
        {
            self.render_command(RenderCommand::SetOceanTime(time as f32 * 0.001));
        }
    }

    pub(in crate::app) fn change_environment_setting(&mut self, direction: i32) {
        match self.environment_selected {
            ui::ENV_ROW_FOG_MODE => {
                self.video.fog_mode = match self.video.fog_mode {
                    FogMode::Off => FogMode::LegacyDrawFog1,
                    FogMode::LegacyDrawFog1 => FogMode::LegacyDrawFog2,
                    FogMode::LegacyDrawFog2 => FogMode::Volumetric,
                    FogMode::Volumetric => FogMode::Off,
                };
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_FOG_STRENGTH => {
                self.set_fog_strength(self.video.fog_strength + direction as f32 * 0.5);
            }
            ui::ENV_ROW_CLOUDS => {
                self.video.clouds = !self.video.clouds;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_TYPE => {
                let current = CloudType::ALL
                    .iter()
                    .position(|kind| *kind == self.video.cloud_type)
                    .unwrap_or(0) as i32;
                let next = (current + direction).rem_euclid(CloudType::ALL.len() as i32) as usize;
                self.video.cloud_type = CloudType::ALL[next];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_QUALITY => {
                self.set_cloud_quality(self.video.cloud_quality + direction as f32 * 0.05)
            }
            ui::ENV_ROW_CLOUD_COVERAGE => {
                self.set_cloud_coverage(self.video.cloud_coverage + direction as f32 * 0.05)
            }
            ui::ENV_ROW_CLOUD_HEIGHT => {
                self.set_cloud_height(self.video.cloud_height + direction as f32 * 128.0)
            }
            ui::ENV_ROW_CLOUD_THICKNESS => {
                self.set_cloud_thickness(self.video.cloud_thickness + direction as f32 * 128.0)
            }
            ui::ENV_ROW_WEATHER_WIND_SPEED => {
                self.set_weather_wind_speed(self.video.weather_wind.speed + direction as f32 * 20.0)
            }
            ui::ENV_ROW_WEATHER_WIND_DIRECTION => self.set_weather_wind_direction(
                self.video.weather_wind.direction + direction as f32 * 15.0,
            ),
            ui::ENV_ROW_WEATHER_GUST_STRENGTH => self
                .set_weather_gust_strength(self.video.weather_wind.gust + direction as f32 * 0.05),
            ui::ENV_ROW_WEATHER_DIRECTION_VARIATION => self.set_weather_direction_variation(
                self.video.weather_wind.shift + direction as f32 * 5.0,
            ),
            ui::ENV_ROW_CLOUD_SHADOWS => {
                self.video.cloud_shadows = !self.video.cloud_shadows;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_RENDER_RESOLUTION => {
                let current = CloudRenderResolution::ALL
                    .iter()
                    .position(|mode| *mode == self.video.cloud_render_resolution)
                    .unwrap_or(1) as i32;
                let next = (current + direction).rem_euclid(CloudRenderResolution::ALL.len() as i32)
                    as usize;
                self.video.cloud_render_resolution = CloudRenderResolution::ALL[next];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_TEMPORAL => {
                self.video.cloud_temporal = !self.video.cloud_temporal;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_RAIN => {
                self.video.rain = !self.video.rain;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_RAIN_INTENSITY => {
                let current = RainIntensity::ALL
                    .iter()
                    .position(|intensity| *intensity == self.video.rain_intensity)
                    .unwrap_or(1) as i32;
                let next =
                    (current + direction).rem_euclid(RainIntensity::ALL.len() as i32) as usize;
                self.video.rain_intensity = RainIntensity::ALL[next];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_PUDDLE_WATER => {
                let current = PuddleQuality::ALL
                    .iter()
                    .position(|quality| *quality == self.video.puddle_quality)
                    .unwrap_or(1) as i32;
                let next =
                    (current + direction).rem_euclid(PuddleQuality::ALL.len() as i32) as usize;
                self.video.puddle_quality = PuddleQuality::ALL[next];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_PUDDLE_SCATTER => {
                self.set_puddle_scatter(self.video.puddle_scatter + direction as f32 * 0.05)
            }
            ui::ENV_ROW_RAIN_GRADE => {
                self.set_rain_grade(self.video.rain_grade + direction as f32 * 0.05)
            }
            ui::ENV_ROW_FOOTPRINTS => {
                let current = FootprintMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.footprints)
                    .unwrap_or(2) as i32;
                let next =
                    (current + direction).rem_euclid(FootprintMode::ALL.len() as i32) as usize;
                self.video.footprints = FootprintMode::ALL[next];
                self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                self.footprints_chosen();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_GRASS => {
                self.video.grass = !self.video.grass;
                // The grass GPU/map data is retained while disabled, so OFF is
                // always instant and ON is instant when this renderer started
                // with grass resources available.
                if !self.video.grass || self.applied_grass {
                    self.render_command(RenderCommand::SetGrassEnabled(self.video.grass));
                }
                self.mark_config_dirty();
                self.console_status = if self.video.grass && !self.applied_grass {
                    "PROCEDURAL GRASS: ON (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)".into()
                } else {
                    format!(
                        "PROCEDURAL GRASS: {} (APPLIED LIVE)",
                        if self.video.grass { "ON" } else { "OFF" }
                    )
                };
            }
            ui::ENV_ROW_OCEAN => {
                self.video.ocean = !self.video.ocean;
                // OFF releases the FFT resources immediately. If the current
                // world already carries its promoted ocean clipmap, ON can also
                // be restored live without rebuilding the renderer.
                if !self.video.ocean || self.applied_ocean {
                    self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                }
                self.mark_config_dirty();
                self.console_status = if self.video.ocean && !self.applied_ocean {
                    "SIMULATED OCEAN: ON (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)".into()
                } else {
                    format!(
                        "SIMULATED OCEAN: {} (APPLIED LIVE)",
                        if self.video.ocean { "ON" } else { "OFF" }
                    )
                };
            }
            ui::ENV_ROW_CLOUD_TUNING => {
                self.cloud_tuning_open = true;
                self.cloud_tuning_selected = 0;
                self.publish_ui();
            }
            ui::ENV_ROW_OCEAN_SETTINGS => {
                self.ocean_settings_open = true;
                self.ocean_selected = 0;
            }
            ui::ENV_ROW_VID_RESTART if self.video_restart_required() => {
                self.apply_video_settings();
                return;
            }
            _ => {}
        }
        self.publish_ui();
    }
}
