//! Post settings.
use crate::renderer::{
    ui, AutoExposureState, CloudHistoryKey, ColorLutPreset, GammaPostUniform, PlanarReflectionMode,
    PostEffects, ReflectionQuality, Renderer,
};

impl Renderer {
    pub(in crate::renderer) fn set_post_effects(&mut self, effects: PostEffects) {
        let old_scene_format = self.scene_format();
        let old_auto_exposure_effective = self.effective_auto_exposure_strength() > 0.0;
        let old_bloom_pyramid = self.glow_mode.enabled() || self.halation_enabled;
        let old_authored_glow = self.glow_mode.authored();
        let old_ssao_enabled = self.ssao_enabled;
        let old_static_bsp_ao_enabled = self.static_bsp_ao_enabled;
        let old_ssr_enabled = self.ssr_enabled;
        let old_reflection_quality = self.reflection_quality;
        let old_reflection_debug = self.reflection_debug_enabled;
        let old_dof_enabled = self.depth_of_field_strength > 0.001;
        let old_motion_blur = self.motion_blur_strength;
        let desired_taa = effects.taa;
        let desired_smaa = effects.smaa && !desired_taa;
        let desired_fxaa = effects.fxaa && !desired_taa && !desired_smaa;
        let taa_changed = self.taa_enabled != desired_taa;
        let rt_taa_changed = self.rt_taa_mode != effects.rt_taa.min(2);
        let smaa_changed = self.smaa_enabled != desired_smaa;
        let color_grade = effects.color_grade.sanitize();
        let split_toning = effects.split_toning.sanitize();
        let lut_strength = if effects.color_lut_strength.is_finite() {
            effects.color_lut_strength.clamp(0.0, 1.0)
        } else {
            self.color_lut_strength
        };
        let mut old_lut_grade = self.color_grade.sanitize();
        old_lut_grade.exposure_ev = 0.0;
        let mut new_lut_grade = color_grade;
        new_lut_grade.exposure_ev = 0.0;
        let grade_changed = (old_lut_grade.lut_active() || new_lut_grade.lut_active())
            && old_lut_grade != new_lut_grade;
        let split_changed = (self.split_toning.active() || split_toning.active())
            && self.split_toning != split_toning;
        let grading_active = new_lut_grade.lut_active() || split_toning.active();
        let lut_changed = self.color_lut_preset != effects.color_lut
            || grade_changed
            || split_changed
            || (grading_active && self.color_lut_strength != lut_strength);
        let old_legacy_fog_effective = self.weather.fog.legacy_effective();
        let weather_history_changed = self.weather.rain.enabled != effects.rain
            || self.weather.rain.intensity != effects.rain_intensity
            || (self.weather.rain.density_scale - effects.rain_density_scale).abs() > 0.0005
            || self.weather.rain.tint != effects.rain_tint
            || self.weather.rain.puddle_quality != effects.puddle_quality
            || (self.weather.rain.puddle_scatter - effects.puddle_scatter).abs() > 0.0005
            || self.weather.snow.enabled != effects.snow
            || (self.weather.snow.density_scale - effects.snow_density_scale).abs() > 0.0005;
        let cloud_resolution_changed =
            self.cloud_render_resolution != effects.cloud_render_resolution;
        // Anything that changes what the cloud history holds. Built from the
        // post-sanitised values the renderer actually stores, so an input the
        // sanitiser rewrites cannot look "changed" on every frame.
        let cloud_history_key = CloudHistoryKey::of(&effects);
        let cloud_history_changed = self.cloud_history_key.as_ref() != Some(&cloud_history_key);
        self.cloud_history_key = Some(cloud_history_key);
        self.hdr_enabled = effects.hdr || self.hdr_output.is_some();
        self.tone_mapping_strength = effects.tone_mapping.clamp(0.0, 1.0);
        self.tone_mapping_enabled = self.tone_mapping_strength > 0.0;
        if let Some(output) = self.hdr_output.as_mut() {
            output.write_settings(&self.queue, effects.hdr_paper_white, effects.hdr_peak, self.tone_mapping_strength);
        }
        self.auto_exposure_strength = effects.auto_exposure.clamp(0.0, 1.0);
        self.auto_exposure_enabled = self.auto_exposure_strength > 0.0;
        self.glow_mode = effects.glow;
        self.halation_enabled = effects.halation;
        self.ssao_enabled = effects.ssao;
        self.static_bsp_ao_enabled = effects.static_bsp_ao;
        self.static_bsp_ao_lightmap = true;
        let old_bake_key = (
            self.static_bsp_ao_samples,
            self.static_bsp_ao_resolution,
            self.static_bsp_ao_strength,
            self.static_bsp_ao_range,
            self.static_bsp_ao_current_cell,
        );
        self.static_bsp_ao_samples = effects.static_bsp_ao_samples;
        self.static_bsp_ao_resolution = effects.static_bsp_ao_resolution;
        self.static_bsp_ao_strength = effects.static_bsp_ao_strength;
        self.static_bsp_ao_range = effects.static_bsp_ao_range;
        self.static_bsp_ao_current_cell = effects.static_bsp_ao_current_cell;
        let bake_quality_changed = old_bake_key
            != (
                self.static_bsp_ao_samples,
                self.static_bsp_ao_resolution,
                self.static_bsp_ao_strength,
                self.static_bsp_ao_range,
                self.static_bsp_ao_current_cell,
            );
        if old_static_bsp_ao_enabled && !self.static_bsp_ao_enabled {
            self.restore_static_ao_lightmaps();
        }
        if old_static_bsp_ao_enabled != self.static_bsp_ao_enabled {
            self.request_static_ao(false);
        } else if self.static_bsp_ao_enabled && bake_quality_changed {
            self.request_static_ao(true);
        }
        self.taa_enabled = desired_taa;
        self.rt_taa_mode = effects.rt_taa.min(2);
        self.smaa_enabled = desired_smaa;
        self.fxaa_enabled = desired_fxaa;
        if self.smaa_enabled {
            self.ensure_smaa_target();
        }
        self.contact_shadows_enabled = effects.contact_shadows;
        self.weather.fog.mode = effects.fog_mode;
        self.weather.fog.strength = effects.fog_strength.clamp(0.0, ui::MAX_FOG_STRENGTH);
        let previous_sun = (self.sun_override, self.sun_yaw, self.sun_pitch,
                            self.sun_intensity, self.sun_color);
        self.sun_override = effects.sun_override;
        self.sun_yaw = effects.sun_yaw.rem_euclid(360.0);
        self.sun_pitch = effects.sun_pitch.clamp(-90.0, 90.0);
        self.sun_intensity = effects.sun_intensity.max(0.0);
        self.sun_shadow_strength = effects.sun_shadow_strength.clamp(0.0, 1.0);
        let old_sun_lighting = self.sun_lighting;
        self.sun_lighting = effects.sun_lighting;
        self.sun_color = effects.sun_color.map(|channel| channel.clamp(0.0, 1.0));
        self.sun_visibility = effects.sun_visibility;
        self.entity_sun_lighting = effects.entity_sun_lighting;
        let old_baked_sun_removal = self.baked_sun_removal;
        self.baked_sun_removal = effects.baked_sun_removal.clamp(0.0, 1.0);
        self.baked_sun_cache = effects.baked_sun_cache;
        let old_baked_sun_shadow_map = self.baked_sun_shadow_map;
        self.baked_sun_shadow_map = effects.baked_sun_shadow_map;
        let old_indirect = (self.baked_indirect_scale, self.baked_indirect_saturation,
            self.baked_indirect_power, self.baked_indirect_highlight_preserve,
            self.baked_sun_edge_softness, self.baked_sun_direct_limit);
        self.baked_indirect_scale = effects.baked_indirect_scale.clamp(0.0, 2.0);
        self.baked_indirect_saturation = effects.baked_indirect_saturation.clamp(0.0, 2.0);
        self.baked_indirect_power = effects.baked_indirect_power.clamp(1.0, 4.0);
        self.baked_indirect_highlight_preserve = effects.baked_indirect_highlight_preserve.clamp(0.0, 1.0);
        self.baked_sun_edge_softness = effects.baked_sun_edge_softness.clamp(0.0, 24.0);
        self.baked_sun_direct_limit = effects.baked_sun_direct_limit.clamp(0.0, 0.9);
        let current_sun = (self.sun_override, self.sun_yaw, self.sun_pitch,
                           self.sun_intensity, self.sun_color);
        if old_baked_sun_removal != self.baked_sun_removal
            || old_baked_sun_shadow_map != self.baked_sun_shadow_map
            || old_indirect != (self.baked_indirect_scale, self.baked_indirect_saturation,
                self.baked_indirect_power, self.baked_indirect_highlight_preserve,
                self.baked_sun_edge_softness, self.baked_sun_direct_limit)
            || previous_sun != current_sun
            || old_sun_lighting != self.sun_lighting
        {
            self.update_lighting_settings();
        }
        self.sky_fractal_override = effects.sky_fractal_override;
        if self.sky_backend != effects.sky_backend.min(1) || self.cloud_backend != effects.cloud_backend.min(1) {
            self.cloud_history_valid = false;
        }
        self.sky_backend = effects.sky_backend.min(1);
        self.blue_water_environment = effects.blue_water_environment.sanitize();
        self.cloud_backend = effects.cloud_backend.min(1);
        let cloud_portal_depth = effects.cloud_portal_depth.min(2);
        if self.cloud_portal_depth != cloud_portal_depth {
            self.cloud_history_valid = false;
        }
        self.cloud_portal_depth = cloud_portal_depth;
        self.clouds_enabled = effects.clouds;
        let auto_exposure_effective =
            self.effective_auto_exposure_strength() > 0.0;
        if old_auto_exposure_effective != auto_exposure_effective || !auto_exposure_effective {
            self.queue.write_buffer(
                &self.auto_exposure_state_buffer,
                0,
                bytemuck::bytes_of(&AutoExposureState {
                    exposure_ev: 0.0,
                    average_luminance: 1.0,
                    target_ev: 0.0,
                    reserved: 0.0,
                }),
            );
        }
        self.cloud_type = effects.cloud_type;
        self.cloud_quality = effects.cloud_quality.clamp(0.0, 1.0);
        self.cloud_coverage = effects.cloud_coverage.clamp(0.0, 1.0);
        self.cloud_height = effects
            .cloud_height
            .clamp(ui::CLOUD_HEIGHT_MIN, ui::CLOUD_HEIGHT_MAX);
        self.cloud_thickness = effects
            .cloud_thickness
            .clamp(ui::CLOUD_THICKNESS_MIN, ui::CLOUD_THICKNESS_MAX);
        let atmospheric_wind = effects.weather_wind.sanitize();
        let atmospheric_wind_changed = self.weather_wind != atmospheric_wind;
        self.weather_wind = atmospheric_wind;
        if atmospheric_wind_changed {
            // There is one atmospheric wind for the rendered world. Keep the
            // ocean simulation on the same source even when this value came from
            // a legacy r_we override rather than the user's r_weatherWind cvar.
            let mut ocean_settings = self.ocean_settings;
            ocean_settings.wind = atmospheric_wind;
            self.set_ocean_settings(ocean_settings);
        }
        self.weather.wind_visuals.strength = if effects.wind_visuals.is_finite() {
            effects.wind_visuals.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let sun = self.active_sun();
        self.grass_renderer.update_environment(
            &self.queue,
            sun,
            self.weather_wind,
            self.pbr_enabled,
        );
        self.cloud_shadows_enabled = effects.cloud_shadows;
        self.cloud_render_resolution = effects.cloud_render_resolution;
        self.cloud_temporal_enabled = effects.cloud_temporal;
        self.cloud_temporal_depth_fix = effects.cloud_temporal_depth_fix;
        self.cloud_shear = effects.cloud_shear;
        self.cloud_base_variation = effects.cloud_base_variation;
        self.cloud_shape_evolution = effects.cloud_shape_evolution;
        self.cloud_terrain_interaction = effects.cloud_terrain_interaction;
        self.cloud_empty_skip = effects.cloud_empty_skip;
        self.cloud_aerial = effects.cloud_aerial;
        if self.cloud_terrain_interaction {
            // Terrain interaction consumes the shared weather heightfield even
            // when rain itself is disabled. Previously this resource was only
            // uploaded by the rain path, leaving the cloud gate inert on dry maps.
            self.ensure_weather_occlusion();
        }
        self.cloud_sky_ambient_enabled = effects.cloud_sky_ambient;
        self.cloud_history_blend = effects.cloud_history_blend;
        self.cloud_motion_reject = effects.cloud_motion_reject;
        self.cloud_history_depth_reject = effects.cloud_history_depth_reject;
        self.cloud_thickness_variation = effects.cloud_thickness_variation;
        self.cloud_size = effects.cloud_size;
        self.weather.rain.enabled = effects.rain && self.blue_water_environment.weather_backend == 0;
        self.weather.rain.native_precipitation=match self.blue_water_environment.weather_backend {
            1=>Some(self.blue_water_environment.rain),2=>Some(0.0),_=>None,
        };
        self.weather.rain.glass_enabled = effects.rain_glass;
        self.weather.rain.intensity = effects.rain_intensity;
        self.weather.rain.density_scale = if effects.rain_density_scale.is_finite() {
            effects.rain_density_scale.clamp(0.0, crate::world_effects::MAX_PARTICLE_CLOUDS as f32)
        } else {
            1.0
        };
        self.weather.rain.tint = effects.rain_tint.map(|channel| channel.clamp(0.0, 1.0));
        self.weather.rain.acid_strength = effects.legacy_acid_rain.clamp(0.0, 1.0);
        self.weather.rain.frozen = effects.rain_frozen;
        self.weather.snow.enabled = effects.snow && self.blue_water_environment.weather_backend == 0;
        self.weather.snow.density_scale = if effects.snow_density_scale.is_finite() {
            effects.snow_density_scale.clamp(0.0, crate::world_effects::MAX_PARTICLE_CLOUDS as f32)
        } else {
            1.0
        };
        self.weather.snow.frozen = effects.snow_frozen;
        self.weather.atmosphere.set_override(crate::world_effects::AtmosphereOverride {
            sand_layers: if self.blue_water_environment.weather_backend==0 {effects.legacy_sand_layers}else{0},
            space_dust_count: if self.blue_water_environment.weather_backend==0 {effects.legacy_space_dust_count}else{0},
            frozen: effects.legacy_weather_frozen,
        });
        self.weather.rain.puddle_quality = effects.puddle_quality;
        self.weather.rain.puddle_scatter = effects.puddle_scatter.clamp(0.0, 1.0);
        self.weather.rain.wet_grade = effects.rain_grade.clamp(0.0, 1.0);
        if cloud_history_changed || !self.clouds_enabled {
            self.cloud_history_valid = false;
            self.cloud_history_read_index = 0;
            self.cloud_temporal_frame_index = 0;
        }
        // Blue Water owns its cloud noise. The legacy CPU bake is only consumed
        // by backend 0; doing it for the native menu blocks the render thread.
        let cloud_resources_changed = self.clouds_enabled
            && self.cloud_backend == 0
            && self.ensure_cloud_noise_resources();
        let new_legacy_fog_effective = self.weather.fog.legacy_effective();
        let legacy_fog_pipeline_changed = old_legacy_fog_effective != new_legacy_fog_effective;
        self.weather.fog.write_legacy_control(&self.queue);
        self.sync_self_legacy_fog();
        self.reflection_quality = effects.reflection_quality;
        self.reflection_debug_enabled = effects.reflection_debug;
        self.ssr_enabled = self.reflection_quality.ssr_enabled();
        self.planar_reflection_mode = if self.reflection_quality == ReflectionQuality::Off {
            PlanarReflectionMode::Off
        } else if self.reflection_quality.promotes_environment_planars() {
            PlanarReflectionMode::Environment
        } else {
            PlanarReflectionMode::Authored
        };
        self.chromatic_aberration_strength = effects.chromatic_aberration.max(0.0);
        self.vignette_enabled = effects.vignette;
        self.film_grain_strength = effects.film_grain_strength.clamp(0.0, 1.0);
        self.motion_blur_strength = effects.motion_blur_strength.clamp(0.0, 1.0);
        self.dof_mode = effects.dof_mode;
        self.depth_of_field_strength = if effects.dof_mode == ui::DofMode::Off {
            0.0
        } else {
            effects.depth_of_field_strength.clamp(0.0, 1.0)
        };
        let old_crt_scope = self.crt_scope;
        self.crt_scope = effects.crt_scope;
        self.crt_style = effects.crt_style;
        self.crt_quality = effects.crt_quality;
        self.crt_reference = effects.crt_reference;
        self.crt_mask = effects.crt_mask;
        self.crt_scanline = effects.crt_scanline.clamp(0.0, 1.0);
        self.crt_sharpness = effects.crt_sharpness.clamp(1.0, 6.0);
        self.crt_mask_intensity = effects.crt_mask_intensity.clamp(0.0, 1.0);
        self.crt_curvature = effects.crt_curvature.clamp(0.0, 0.25);
        self.crt_corner = effects.crt_corner.clamp(0.0, 11.0);
        if self.crt_scope == ui::CrtScope::Off {
            // CRT is genuinely zero-resource when disabled, even after it has
            // previously been used in this session.
            self.crt_resources = None;
        } else {
            self.ensure_crt_resources();
        }
        if old_crt_scope != self.crt_scope {
            rverbose!(1, "CRT: {}", self.crt_scope.label());
        }
        self.dof_quality = effects.dof_quality;
        if (old_motion_blur - self.motion_blur_strength).abs() > 0.001 {
            self.motion_blur_runtime_scale = 0.0;
        }
        if old_dof_enabled != (self.depth_of_field_strength > 0.001) {
            self.dof_focus_valid = false;
        }
        self.color_lut_strength = lut_strength;
        self.color_grade = color_grade;
        self.split_toning = split_toning;
        self.color_lut_effective_strength = if grading_active {
            // Film-LUT intensity and all grading controls are already baked into
            // one cube, so the runtime lookup must be applied at full strength.
            1.0
        } else if effects.color_lut != ColorLutPreset::Off {
            lut_strength
        } else {
            0.0
        };
        self.color_lut_preset = effects.color_lut;
        let lut_binding_changed = lut_changed && self.rebuild_color_grading_lut();
        if lut_changed {
            rverbose!(1, "Color LUT: {}", self.color_lut_preset.label());
        }
        self.queue.write_buffer(
            &self.gamma_post_buffer,
            0,
            bytemuck::bytes_of(&GammaPostUniform {
                values: [
                    self.output_gamma(),
                    self.color_lut_effective_strength,
                    if self.vignette_enabled { 1.0 } else { 0.0 },
                    self.color_grade.exposure_ev,
                ],
                flags: [if self.hdr_enabled { 1.0 } else { 0.0 }, if self.hdr_output.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0],
            }),
        );
        self.rebuild_frame_plan();
        if taa_changed || rt_taa_changed || smaa_changed || weather_history_changed {
            self.history_valid = false;
            self.taa_frame_index = 0;
            self.history_read_index = 0;
            self.ssr_history_valid = false;
            self.ssr_history_read_index = 0;
            self.ssr_frame_index = 0;
        }
        if self.gameplay_scene_format() == wgpu::TextureFormat::Rgba16Float
            && !self.hdr_supported_msaa.contains(&self.msaa_samples)
        {
            self.msaa_samples = 1;
        }
        let bloom_pyramid_changed =
            old_bloom_pyramid != (self.glow_mode.enabled() || self.halation_enabled);
        let authored_glow_resources_changed = old_authored_glow != self.glow_mode.authored();
        let ssao_resources_changed = old_ssao_enabled != self.ssao_enabled;
        let ssr_resources_changed = old_ssr_enabled != self.ssr_enabled;
        let reflection_policy_changed = old_reflection_quality != self.reflection_quality;
        let reflection_debug_changed = old_reflection_debug != self.reflection_debug_enabled;
        if reflection_policy_changed {
            self.rebuild_planar_reflection_resources();
            // Planar activity is itself part of the fast/advanced frame-plan
            // decision, so rebuild once more after the resource state is final.
            self.rebuild_frame_plan();
            self.update_lighting_settings();
            self.activate_world_pipeline_variant();
            self.ssr_history_valid = false;
            self.ssr_history_read_index = 0;
            self.ssr_frame_index = 0;
        }
        if reflection_debug_changed {
            self.history_valid = false;
            self.ssr_history_valid = false;
        }
        let dof_resources_changed = old_dof_enabled != (self.depth_of_field_strength > 0.001);
        if old_scene_format != self.scene_format()
            || bloom_pyramid_changed
            || authored_glow_resources_changed
            || dof_resources_changed
            || ssao_resources_changed
            || ssr_resources_changed
            || cloud_resolution_changed
        {
            self.request_scene_rebuild();
        } else {
            if legacy_fog_pipeline_changed {
                self.activate_world_pipeline_variant();
            }
            if cloud_resources_changed {
                self.rebuild_post_bind_group();
            } else if lut_binding_changed {
                self.rebuild_color_lut_bind_groups();
            }
            self.update_post_uniform();
        }
        self.rebuild_ui();
    }
}
