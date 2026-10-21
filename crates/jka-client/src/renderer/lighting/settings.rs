//! Lighting settings.
use crate::renderer::{
    project_blob_shadow_mark, DynamicLightsMode, DynamicModelSurface, EntityAmbientLightingMode,
    Ghoul2BatchMode, PbrSettings, Renderer, Vec3, PBR_PROFILE_MATERIALS,
    PBR_PROFILE_PARALLAX_OCCLUSION,
};

impl Renderer {
    pub(in crate::renderer) fn set_gpu_timings(&mut self, enabled: bool) {
        self.gpu_profiler.set_enabled(enabled);
        rverbose!(
            1,
            "GPU timestamps: {}",
            if enabled {
                if self.gpu_profiler.available() {
                    "enabled"
                } else {
                    "unavailable on this adapter"
                }
            } else {
                "disabled"
            }
        );
    }

    pub(in crate::renderer) fn set_ghoul2_batch_draws(&mut self, mode: Ghoul2BatchMode) {
        self.dynamic_model_renderer.set_ghoul2_batch_draws(mode);
    }

    pub(in crate::renderer) fn set_dynamic_light_falloff(&mut self, mode: u32) {
        let mode = mode.min(1);
        if self.dynamic_light_falloff == mode {
            return;
        }
        self.dynamic_light_falloff = mode;
        self.update_lighting_settings();
    }

    pub(in crate::renderer) fn set_dynamic_light_debug(&mut self, enabled: bool) {
        if self.dynamic_light_debug == enabled {
            return;
        }
        self.dynamic_light_debug = enabled;
        // The baseline BSP path has no local-light bindings. Debug always uses
        // the advanced receiver shader so the view can be toggled without a
        // map reload or pipeline mutation.
        self.rebuild_frame_plan();
        self.update_lighting_settings();
        self.history_valid = false;
    }

    pub(in crate::renderer) fn set_rt_samples(&mut self, samples: u32) {
        let samples = if matches!(samples, 1 | 2 | 4) {
            samples
        } else {
            1
        };
        if self.rt_samples == samples {
            return;
        }
        self.rt_samples = samples;
        if let Some(rt) = self.ray_traced_shadows.as_mut() { rt.sun_shadow_history.invalidate(); }
        if self.hardware_rt_requested() {
            self.update_lighting_settings();
            self.history_valid = false;
        }
    }



    pub(in crate::renderer) fn set_rt_trace_rate(&mut self, rate: u32) {
        let rate = if matches!(rate, 1 | 2 | 4) { rate } else { 1 };
        if self.rt_trace_rate == rate {
            return;
        }
        self.rt_trace_rate = rate;
        if let Some(rt) = self.ray_traced_shadows.as_mut() {
            rt.sun_shadow_history.invalidate();
        }
        self.history_valid = false;
        self.rebuild_frame_plan();
    }

    pub(in crate::renderer) fn set_dynamic_lighting(&mut self, mode: DynamicLightsMode) {
        self.dynamic_lights_mode = mode;
        // Direct RT lighting shares Forward+ light selection and radiometry;
        // only visibility changes. Legacy/Vertex/Clustered Lite stay separate.
        self.clustered_lighting_enabled = matches!(
            mode,
            DynamicLightsMode::PerPixelForwardPlus | DynamicLightsMode::RayTracedHardware
        );
        if let Some(world) = &mut self.world {
            world.local_shadows.selection_position = None;
        }
        self.upload_light_buffer();
        self.rebuild_frame_plan();
        self.update_lighting_settings();
        self.activate_world_pipeline_variant();
        rverbose!(1, "Dynamic lighting requested={mode:?}; world shaders {}; enable r_verbose 1 to see activation", if self.world_variant_pending.is_some() { "compiling" } else { "active" });
        if mode == DynamicLightsMode::RayTracedHardware {
            self.log_rt_dynamic_lighting_status();
        }
    }

    /// Report the *installed* lighting state, not just the requested cvar.  A
    /// background world-variant compile can leave the previous raster shader
    /// active for several frames after switching to RayTracedHardware.
    pub(in crate::renderer) fn log_rt_dynamic_lighting_status(&self) {
        if self.dynamic_lights_mode != DynamicLightsMode::RayTracedHardware {
            return;
        }
        if self.world_variant_pending.is_some() {
            rverbose!(
                1,
                "Hardware RT Lighting: pending shader activation (previous compatible world variant remains active)"
            );
        } else if self.hardware_rt_active() && self.ray_traced_shadows.is_some() {
            rverbose!(
                1,
                "Hardware RT Lighting: ACTIVE; static={} transient={} total={} brightness={:.2} vertex_light={} (sun shadow mode independent)",
                self.active_static_light_count(),
                self.active_transient_light_count(),
                self.active_light_count(),
                self.dynamic_light_brightness,
                self.classic_vertex_light,
            );
        } else if !self.ray_tracing_supported {
            rverbose!(1, "Hardware RT Lighting: unavailable - GPU/backend lacks hardware ray-query support; using Forward+");
        } else if self.ray_traced_shadows.is_none() {
            rverbose!(1, "Hardware RT Lighting: unavailable - no RT acceleration structures for this map; using Forward+");
        } else {
            rverbose!(1, "Hardware RT Lighting: not active - renderer is using a non-RT shader variant");
        }
    }

    pub(in crate::renderer) fn classic_world_render_flags(&self) -> u32 {
        u32::from(self.classic_fullbright)
            | (u32::from(self.classic_vertex_light) << 1)
            | (u32::from(self.classic_lightmap_only) << 2)
    }

    pub(in crate::renderer) fn set_classic_world_lighting(
        &mut self,
        fullbright: bool,
        vertex_light: bool,
        lightmap_only: bool,
    ) {
        let vertex_light_changed = self.classic_vertex_light != vertex_light;
        self.classic_fullbright = fullbright;
        self.classic_vertex_light = vertex_light;
        self.classic_lightmap_only = lightmap_only;
        if vertex_light_changed {
            // OpenJK clears RE_AddLightToScene dlights while r_vertexLight is on.
            // Keep that classic interaction for runtime/transient lights without
            // conflating it with the separate Dynamic Lights > Vertex mode.
            self.upload_light_buffer();
            self.update_lighting_settings();
        }
    }

    pub(in crate::renderer) fn set_map_light_simulation(&mut self, enabled: bool) {
        if self.map_light_simulation_enabled == enabled {
            return;
        }
        self.map_light_simulation_enabled = enabled;
        if let Some(world) = &mut self.world {
            world.local_shadows.selection_position = None;
        }
        self.upload_light_buffer();
        self.rebuild_frame_plan();
        self.update_lighting_settings();
        self.activate_world_pipeline_variant();
        rverbose!(
            1,
            ".map light simulation: {}{}",
            if enabled { "enabled" } else { "disabled" },
            if self.world.as_ref().is_some_and(|world| !world.source_map) {
                " (current world is a compiled BSP; setting is dormant)"
            } else {
                ""
            }
        );
    }

    pub(in crate::renderer) fn set_emissive_area_lights(&mut self, enabled: bool) {
        self.emissive_area_lights_enabled = enabled;
        if let Some(world) = &mut self.world {
            // Force the local-shadow cache to drop/reconsider area emitters.
            world.local_shadows.selection_position = None;
        }
        self.upload_light_buffer();
        self.rebuild_frame_plan();
        self.update_lighting_settings();
        self.activate_world_pipeline_variant();
    }

    pub(in crate::renderer) fn set_shader_light_promotion(&mut self, enabled: bool, mask: u32) {
        let mask = mask & crate::materials::SHADER_LIGHT_ALL;
        if self.shader_light_promotion_enabled == enabled && self.shader_light_promotion_mask == mask {
            return;
        }
        self.shader_light_promotion_enabled = enabled;
        self.shader_light_promotion_mask = mask;
        if let Some(world) = &mut self.world {
            world.local_shadows.selection_position = None;
        }
        self.upload_light_buffer();
        self.rebuild_frame_plan();
        self.update_lighting_settings();
        self.activate_world_pipeline_variant();
        self.history_valid = false;
    }

    pub(in crate::renderer) fn set_model_brightness(&mut self, brightness: f32) {
        self.dynamic_model_renderer
            .set_model_brightness(&self.queue, brightness);
    }

    /// Feeds the entity depth prepass the sun direction used by the projected
    /// cloud shadow, so models give post a smooth sun-facing value instead of a
    /// per-triangle depth-derived one. Off (w = 0) unless clouds and cloud
    /// shadows are both on; the GPU write is skipped while nothing changed.
    pub(in crate::renderer) fn sync_entity_cloud_shadow_sun(&mut self) {
        let sun = if self.clouds_enabled && self.cloud_shadows_enabled {
            let toward_sun = -Vec3::from_array(self.active_sun().direction);
            toward_sun
                .try_normalize()
                .map_or([0.0; 4], |d| [d.x, d.y, d.z, 1.0])
        } else {
            [0.0; 4]
        };
        self.dynamic_model_renderer
            .set_cloud_shadow_sun(&self.queue, sun);
    }

    pub(in crate::renderer) fn set_dynamic_light_brightness(&mut self, brightness: f32) {
        let brightness = if brightness.is_finite() {
            brightness.max(0.0)
        } else {
            1.0
        };
        if (self.dynamic_light_brightness - brightness).abs() < f32::EPSILON {
            return;
        }
        self.dynamic_light_brightness = brightness;
        self.upload_light_buffer();
    }

    pub(in crate::renderer) fn set_entity_ambient_lighting(
        &mut self,
        mode: EntityAmbientLightingMode,
    ) {
        self.irradiance_volume_enabled =
            matches!(mode, EntityAmbientLightingMode::BevyIrradianceVolume);
        self.dynamic_model_renderer
            .set_entity_ambient_lighting(&self.queue, mode);
        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
    }

    pub(in crate::renderer) fn set_voxel_probe_gi(&mut self, enabled: bool) {
        self.voxel_probe_gi_enabled = enabled;
        self.rebuild_frame_plan();
        self.update_lighting_settings();
        self.activate_world_pipeline_variant();
    }

    pub(in crate::renderer) fn set_tinted_glass(&mut self, enabled: bool) {
        let enabled = enabled && crate::renderer::lighting::rt_diagnostics::config().receiver.glass;
        if self.tinted_glass_enabled != enabled {
            self.tinted_glass_enabled = enabled;
            // Opaque-classified lightfilter panes must be removed/reinstated
            // in the raster depth cubemap on either toggle.
            if let Some(world) = self.world.as_mut() {
                for slot in 0..world.local_shadows.cache_entries.len() {
                    if world.local_shadows.cache_entries[slot].light_index.is_some()
                        && !world.local_shadows.pending_slots.contains(&slot)
                    {
                        world.local_shadows.pending_slots.push(slot);
                    }
                }
            }
            self.update_lighting_settings();
            // Local RT light queries are traced every frame; no sun-shadow
            // history is used for this RGB signal, so no cache flush is needed.
        }
    }

    pub(in crate::renderer) fn set_tinted_glass_debug(&mut self, mode: u32) {
        self.tinted_glass_debug = mode.min(2);
        self.update_lighting_settings();
    }

    pub(in crate::renderer) fn set_local_light_shadows(&mut self, enabled: bool) {
        self.local_light_shadows_enabled = enabled;
        self.rebuild_frame_plan();
        self.update_lighting_settings();
        self.activate_world_pipeline_variant();
    }

    pub(in crate::renderer) fn write_material_enhancement_settings(&self) {
        let settings = PbrSettings {
            values: [
                u32::from(self.pbr_enabled && PBR_PROFILE_MATERIALS),
                u32::from(self.pbr_enabled && self.pom_enabled && PBR_PROFILE_PARALLAX_OCCLUSION),
                40,
                0,
            ],
            deluxe: [
                f32::from(self.pbr_enabled && self.deluxe_mapping_enabled),
                self.deluxe_specular,
                f32::from(self.detail_texture_fade),
                self.detail_texture_fade_distance,
            ],
        };
        self.queue
            .write_buffer(&self.pbr_settings_buffer, 0, bytemuck::bytes_of(&settings));
    }

    pub(in crate::renderer) fn set_pom_enabled(&mut self, enabled: bool) {
        if self.pom_enabled == enabled {
            return;
        }
        self.pom_enabled = enabled;
        self.write_material_enhancement_settings();
        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
        rverbose!(1, "POM: {}", if enabled { "ON" } else { "OFF" });
    }

    pub(in crate::renderer) fn set_pbr_settings(
        &mut self,
        enabled: bool,
        deluxe_mapping: bool,
        deluxe_specular: f32,
    ) {
        self.pbr_enabled = enabled;
        self.deluxe_mapping_enabled = deluxe_mapping;
        self.deluxe_specular = deluxe_specular.clamp(0.0, 1.0);
        self.write_material_enhancement_settings();
        let sun = self.active_sun();
        self.grass_renderer.update_environment(
            &self.queue,
            sun,
            self.weather_wind,
            self.pbr_enabled,
        );
        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
        rverbose!(
            1,
            "PBR profile: {} (companions={} POM={} deluxe={} deluxe_spec={:.2})",
            if enabled { "ON" } else { "OFF" },
            u8::from(enabled && PBR_PROFILE_MATERIALS),
            u8::from(enabled && self.pom_enabled && PBR_PROFILE_PARALLAX_OCCLUSION),
            u8::from(enabled && deluxe_mapping),
            self.deluxe_specular,
        );
    }

    /// OpenJK marks blob shadows onto the *rendered* surfaces (CG_ImpactMark ->
    /// R_MarkFragments): the shadow polygon is clipped against every nearby facing
    /// markable triangle, so it follows slopes, steps and floor detail that sits
    /// off the collision floor, and skips nomarks/noimpact/fog shaders and model
    /// triangle soup. This is the same projector the saber marks use; only the
    /// quad set-up and the dynamic-mesh output are blob specific. One small
    /// query per blob, batched into one mesh. Costs nothing without blobs.
    pub(in crate::renderer) fn build_blob_shadow_marks(
        &mut self,
        dynamic_models: &[DynamicModelSurface],
    ) {
        let mut vertices = std::mem::take(&mut self.dynamic_model_renderer.blob_mesh_vertices);
        let mut indices = std::mem::take(&mut self.dynamic_model_renderer.blob_mesh_indices);
        vertices.clear();
        indices.clear();
        let surfaces = self
            .world
            .as_ref()
            .and_then(|world| world.mark_surfaces.clone());
        if let Some(surfaces) = surfaces {
            for surface in dynamic_models {
                let Some(sprites) = surface
                    .fx_gpu_sprites
                    .as_ref()
                    .filter(|sprites| sprites.blob_shadow)
                else {
                    continue;
                };
                for request in sprites.instances.iter() {
                    project_blob_shadow_mark(
                        &surfaces,
                        &mut self.blob_mark_buffer,
                        Vec3::new(request.origin[0], request.origin[1], request.origin[2]),
                        Vec3::new(request.left[0], request.left[1], request.left[2]),
                        Vec3::new(request.up[0], request.up[1], request.up[2]),
                        request.origin[3],
                        &mut vertices,
                        &mut indices,
                    );
                }
            }
        }
        self.dynamic_model_renderer.blob_mesh_vertices = vertices;
        self.dynamic_model_renderer.blob_mesh_indices = indices;
    }
}
