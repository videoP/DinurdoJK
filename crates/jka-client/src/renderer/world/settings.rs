//! World settings.
use crate::renderer::{
    auto_detail_fallback_index, create_fast_world_wireframe_pipeline, create_wireframe_pipeline,
    create_world_pipeline_variant_from_plan, create_world_wireframe_pipeline, pipeline_hash,
    required_auto_detail_indices, try_load_texture_asset_from_search_path, ui, upload_texture,
    AssetSearchPath, CullDebugMode, DetailTextureMode, DynamicLightsMode, DynamicShadowsMode,
    GrassPreparedDraw, HashMap, Instant, JumpShadeState, Path, PipelineJobKey,
    PlanarReflectionDebugMode, PreparedSurfaceSpriteEffects, PvsMode, Renderer,
    WorldPipelineCompilePlan, WorldPipelineVariant, WorldRenderPath, WorldShaderFamily,
    WorldShaderVariantKey, AUTO_DETAIL_TEXTURE_FILES, PBR_PROFILE_COMPANION_SAMPLER_TRILINEAR,
    PBR_PROFILE_MATERIALS, PBR_PROFILE_PARALLAX_OCCLUSION, PBR_PROFILE_POM_ADAPTIVE_STEPS,
    PBR_PROFILE_POM_MIP_AWARE, PBR_PROFILE_SHARED_MATERIAL_EVAL, PBR_PROFILE_SHARED_TANGENT_FRAME,
    PBR_PROFILE_VERTEX_LIGHTGRID,
};

impl Renderer {
    pub(in crate::renderer) fn set_picmip(&mut self, picmip: u32) {
        self.picmip = picmip.min(16);
    }

    pub(in crate::renderer) fn ensure_auto_detail_textures_for_plan(&mut self, plan: &[u8]) {
        if !self.detail_texture_auto {
            return;
        }
        let mut needed = required_auto_detail_indices(plan);
        if needed.is_empty() {
            return;
        }
        needed.insert(auto_detail_fallback_index());
        needed.retain(|index| {
            AUTO_DETAIL_TEXTURE_FILES
                .get(usize::from(*index))
                .is_some_and(|name| !self.detail_auto_textures.contains_key(*name))
        });
        if needed.is_empty() {
            return;
        }

        let started = Instant::now();
        let mut assets =
            match AssetSearchPath::open_game(&self.base, self.detail_texture_game.as_deref()) {
                Ok(assets) => assets,
                Err(error) => {
                    eprintln!("AUTO BSP detail texture asset search: {error}");
                    return;
                }
            };
        const PREFIX: &str = "textures/japro/detail/";
        let available = assets
            .names()
            .filter(|name| name.to_ascii_lowercase().starts_with(PREFIX))
            .filter_map(|name| {
                name.rsplit('/')
                    .next()
                    .map(|leaf| (leaf.to_ascii_lowercase(), name.to_owned()))
            })
            .collect::<HashMap<_, _>>();

        let mut loaded = 0usize;
        for index in needed {
            let Some(wanted) = AUTO_DETAIL_TEXTURE_FILES.get(usize::from(index)).copied() else {
                continue;
            };
            let Some(path) = available.get(wanted) else {
                eprintln!("AUTO BSP detail texture: missing {PREFIX}{wanted}");
                continue;
            };
            let Some(data) = try_load_texture_asset_from_search_path(
                &mut assets,
                path,
                false,
                true,
                true,
                "AUTO BSP detail texture",
            ) else {
                continue;
            };
            let image = upload_texture(&self.device, &self.queue, &data);
            self.baked_brightness.register(&image._texture);
            self.detail_auto_textures.insert(wanted.to_owned(), image);
            loaded += 1;
        }
        rverbose!(
            1,
            "AUTO BSP detail cache: loaded {} texture(s) lazily in {:.1} ms ({} cached)",
            loaded,
            started.elapsed().as_secs_f64() * 1000.0,
            self.detail_auto_textures.len(),
        );
    }

    pub(in crate::renderer) fn set_detail_textures(&mut self, mode: DetailTextureMode) {
        if self.detail_textures_mode == mode {
            return;
        }
        self.detail_textures_mode = mode;

        // If a map was loaded while details were disabled, populate only the
        // textures selected by that world's MATERIAL_* plan when details are
        // enabled. The world keeps the plan specifically so this stays lazy.
        if mode != DetailTextureMode::Off {
            let plan = self
                .world
                .as_ref()
                .map(|world| world.detail_texture_by_base.clone());
            if let Some(plan) = plan {
                self.ensure_auto_detail_textures_for_plan(&plan);
                if let Some(mut world) = self.world.take() {
                    self.rebuild_world_surface_bind_groups(&mut world);
                    self.world = Some(world);
                    self.sync_baked_brightness_assets();
                }
            }
        }

        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
        rverbose!(
            1,
            "Detail textures: {} (textures/japro/detail; eligible opaque BSP stages only)",
            mode.label()
        );
    }

    pub(in crate::renderer) fn set_jump_shade(&mut self, state: JumpShadeState) {
        let engaged_changed = self.jump_shade.engaged() != state.engaged();
        self.jump_shade = state;
        if engaged_changed {
            // Compiles (once, then cached) the world variant that carries the tint,
            // and routes off the fast baseline shader, which does not have it.
            self.rebuild_frame_plan();
            self.activate_world_pipeline_variant();
        }
    }

    pub(in crate::renderer) fn set_detail_texture(&mut self, _path: &str, game: Option<&Path>) {
        // The image choice is permanently AUTO. A game/mod change invalidates
        // only the cached DT_* images; do not repopulate them until a loaded
        // world actually needs them.
        self.detail_texture_auto = true;
        let next_game = game.map(Path::to_path_buf);
        if self.detail_texture_game == next_game {
            return;
        }
        self.detail_texture_game = next_game;
        self.detail_auto_textures.clear();

        let plan = self
            .world
            .as_ref()
            .map(|world| world.detail_texture_by_base.clone());
        if self.detail_textures_mode != DetailTextureMode::Off {
            if let Some(plan) = plan {
                self.ensure_auto_detail_textures_for_plan(&plan);
                if let Some(mut world) = self.world.take() {
                    self.rebuild_world_surface_bind_groups(&mut world);
                    self.world = Some(world);
                    self.sync_baked_brightness_assets();
                }
            }
        }
        rverbose!(1, "Detail texture image: AUTO (lazy MATERIAL_* selection)");
    }

    pub(in crate::renderer) fn set_detail_texture_fade(&mut self, enabled: bool, distance: f32) {
        let distance = distance.clamp(64.0, 8192.0);
        if self.detail_texture_fade == enabled
            && (self.detail_texture_fade_distance - distance).abs() < 0.01
        {
            return;
        }
        self.detail_texture_fade = enabled;
        self.detail_texture_fade_distance = distance;
        self.write_material_enhancement_settings();
        rverbose!(
            1,
            "Detail texture distance fade: {} (distance {:.0})",
            if enabled { "ON" } else { "OFF" },
            distance,
        );
    }

    pub(in crate::renderer) fn set_wireframe_mask(&mut self, mask: u32) {
        self.wireframe_mask = if self.wireframe_supported {
            mask & ui::wireframe::ALL
        } else {
            0
        };
    }

    #[inline]
    pub(in crate::renderer) fn wireframe_requires_dynamic_class_split(&self) -> bool {
        if self.wireframe_mask == 0 {
            return false;
        }
        let dynamic_mask =
            ui::wireframe::PLAYERS | ui::wireframe::ENTITIES | ui::wireframe::EFFECTS;
        let selected = self.wireframe_mask & dynamic_mask;
        selected != 0 && selected != dynamic_mask
    }

    pub(in crate::renderer) fn ensure_prepared_wireframe_pipelines(
        &mut self,
        prepared_grass: Option<&GrassPreparedDraw>,
        prepared_surface_sprite_effects: Option<&PreparedSurfaceSpriteEffects>,
    ) {
        if self.wireframe_mask == 0 {
            return;
        }
        let scene_format = self.scene_format();
        let samples = self.msaa_samples;
        if self.wireframe_mask & ui::wireframe::GRASS != 0 {
            if let Some(prepared) = prepared_grass {
                self.grass_renderer.ensure_wireframe_pipeline_for_draw(
                    &mut self.pipeline_jobs,
                    &self.device,
                    scene_format,
                    samples,
                    prepared,
                );
            }
        }
        if self.wireframe_mask & ui::wireframe::EFFECTS != 0
            && prepared_surface_sprite_effects.is_some()
        {
            self.surface_sprite_effect_renderer
                .ensure_wireframe_pipeline(&mut self.pipeline_jobs, &self.device);
        }
        let dynamic_mask =
            ui::wireframe::PLAYERS | ui::wireframe::ENTITIES | ui::wireframe::EFFECTS;
        if self.wireframe_mask & dynamic_mask != 0 {
            self.dynamic_model_renderer.ensure_wireframe_pipelines(
                &mut self.pipeline_jobs,
                &self.device,
                scene_format,
                samples,
            );
        }
    }

    pub(in crate::renderer) fn ensure_wireframe_pipelines(&mut self) {
        let scene_format = self.scene_format();
        self.debug_volumes.ensure(
            &mut self.pipeline_jobs,
            &self.device,
            &self.wireframe_pipeline_layout,
            &self.debug_volume_shader,
            scene_format,
            self.msaa_samples,
        );
        if self.wireframe_mask == 0 {
            return;
        }

        let samples = self.msaa_samples;
        if self.world.is_some()
            && self.wireframe_mask & (ui::wireframe::MAP | ui::wireframe::ENTITIES) != 0
            && self.wireframe_pipeline.is_none()
        {
            let key = PipelineJobKey::new(
                "world-wireframe",
                0,
                pipeline_hash(&(scene_format, samples)),
            );
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.wireframe_pipeline = Some(pipeline);
            } else {
                let device = self.device.clone();
                let layout = self.wireframe_pipeline_layout.clone();
                let shader = self.wireframe_shader.clone();
                self.pipeline_jobs.request(key, "world wireframe", move || {
                    create_wireframe_pipeline(&device, &layout, &shader, scene_format, samples)
                });
            }
        }

        let has_deformation = self
            .world
            .as_ref()
            .map_or(false, |world| world.snow_shell.has_snow());
        if self.frame_plan.world_path == WorldRenderPath::FastBaseline
            && self.wireframe_mask & ui::wireframe::DEFORMATION != 0
            && has_deformation
            && self.fast_world_wireframe_pipeline.is_none()
        {
            let key = PipelineJobKey::new(
                "fast-world-wireframe",
                0,
                pipeline_hash(&(scene_format, samples)),
            );
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.fast_world_wireframe_pipeline = Some(pipeline);
            } else {
                let device = self.device.clone();
                let layout = self.fast_world_pipeline_layout.clone();
                let shader = self.fast_world_shader.clone();
                self.pipeline_jobs
                    .request(key, "fast world wireframe", move || {
                        create_fast_world_wireframe_pipeline(
                            &device,
                            &layout,
                            &shader,
                            scene_format,
                            samples,
                        )
                    });
            }
        }

        let active_key = self
            .world
            .as_ref()
            .map(|world| world.active_pipeline_variant);
        let has_ocean_clipmap = self.ocean_enabled
            && self.world.as_ref().map_or(false, |world| {
                world
                    .coarse_batches
                    .iter()
                    .any(|batch| batch.ocean_clipmap.is_some())
                    || world
                        .full_batches
                        .iter()
                        .any(|batch| batch.ocean_clipmap.is_some())
            });
        let needs_deformed_world = self.frame_plan.world_path == WorldRenderPath::Advanced
            && ((self.wireframe_mask & ui::wireframe::OCEAN != 0 && has_ocean_clipmap)
                || (self.wireframe_mask & ui::wireframe::DEFORMATION != 0 && has_deformation));
        if needs_deformed_world {
            if let Some(variant) = active_key {
                if !self.world_wireframe_pipelines.contains_key(&variant) {
                    let key = PipelineJobKey::new(
                        "world-wireframe-variant",
                        pipeline_hash(&variant) as u32,
                        pipeline_hash(&(scene_format, samples, variant)),
                    );
                    if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                        self.world_wireframe_pipelines.insert(variant, pipeline);
                    } else {
                        let chosen = if variant.ray_traced_shadows {
                            self.ray_traced_shadows
                                .as_ref()
                                .map(|rt| match variant.family() {
                                    WorldShaderFamily::Lean => (
                                        rt.world_pipeline_layout_lean.clone(),
                                        rt.world_shader_lean.clone(),
                                    ),
                                    WorldShaderFamily::Enhanced => {
                                        (rt.world_pipeline_layout.clone(), rt.world_shader.clone())
                                    }
                                })
                        } else {
                            Some(match variant.family() {
                                WorldShaderFamily::Lean => (
                                    self.world_pipeline_layout_lean.clone(),
                                    self.world_shader_lean.clone(),
                                ),
                                WorldShaderFamily::Enhanced => (
                                    self.world_pipeline_layout.clone(),
                                    self.world_shader.clone(),
                                ),
                            })
                        };
                        if let Some((layout, shader)) = chosen {
                            let device = self.device.clone();
                            self.pipeline_jobs.request(
                                key,
                                "deformed world wireframe",
                                move || {
                                    create_world_wireframe_pipeline(
                                        &device,
                                        &layout,
                                        &shader,
                                        scene_format,
                                        samples,
                                        variant,
                                    )
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    pub(in crate::renderer) fn set_pvs_mode(&mut self, mode: PvsMode) {
        self.pvs_mode = mode;
    }

    pub(in crate::renderer) fn set_gamma(&mut self, gamma: f32) {
        if !gamma.is_finite() {
            return;
        }
        let gamma = gamma.clamp(0.5, 3.0);
        if (self.gamma - gamma).abs() < f32::EPSILON {
            return;
        }
        self.gamma = gamma;
        let output_gamma = self.output_gamma();
        if self.baked_brightness.enabled || self.baked_brightness.busy {
            self.baked_brightness
                .set_target(self.baked_brightness.enabled, gamma);
        }
        if (self.gamma_method != crate::gamma::GammaMethod::Hardware && !self.baked_brightness.enabled)
            || self.output_gamma() != output_gamma {
            self.refresh_output_gamma();
        }
    }

    pub(in crate::renderer) fn map_light_simulation_active_for(&self, source_map: bool) -> bool {
        self.map_light_simulation_enabled && source_map
    }

    pub(in crate::renderer) fn map_light_simulation_active(&self) -> bool {
        self.map_light_simulation_active_for(
            self.world.as_ref().is_some_and(|world| world.source_map),
        )
    }

    pub(in crate::renderer) fn point_lighting_enabled_for(&self, source_map: bool) -> bool {
        self.clustered_lighting_enabled || self.map_light_simulation_active_for(source_map)
    }

    pub(in crate::renderer) fn point_lighting_enabled(&self) -> bool {
        self.clustered_lighting_enabled || self.map_light_simulation_active()
    }

    pub(in crate::renderer) fn static_point_lighting_enabled_for(&self, source_map: bool) -> bool {
        if source_map {
            self.map_light_simulation_active_for(true)
        } else {
            self.clustered_lighting_enabled
        }
    }

    pub(in crate::renderer) fn static_point_lighting_enabled(&self) -> bool {
        let source_map = self.world.as_ref().is_some_and(|world| world.source_map);
        self.static_point_lighting_enabled_for(source_map)
    }

    pub(in crate::renderer) fn world_shader_variant_key(&self) -> WorldShaderVariantKey {
        let source_map = self.world.as_ref().is_some_and(|world| world.source_map);
        self.world_shader_variant_key_for_source_map(source_map)
    }

    pub(in crate::renderer) fn world_shader_variant_key_for_source_map(
        &self,
        source_map: bool,
    ) -> WorldShaderVariantKey {
        let legacy_dlights = self.dynamic_lights_mode == DynamicLightsMode::Legacy;
        let vertex_dlights = self.dynamic_lights_mode == DynamicLightsMode::Vertex;
        let clustered_lite_dlights = self.dynamic_lights_mode == DynamicLightsMode::ClusteredLite;
        let point_lights = self.point_lighting_enabled_for(source_map);
        let map_light_simulation = self.map_light_simulation_active_for(source_map);
        let shadowable_local_lights = point_lights || self.emissive_area_lights_enabled;
        WorldShaderVariantKey {
            pbr: self.pbr_enabled && PBR_PROFILE_MATERIALS,
            pom: self.pbr_enabled && self.pom_enabled && PBR_PROFILE_PARALLAX_OCCLUSION,
            pbr_shared_material_eval: self.pbr_enabled && PBR_PROFILE_SHARED_MATERIAL_EVAL,
            pbr_shared_tangent_frame: self.pbr_enabled && PBR_PROFILE_SHARED_TANGENT_FRAME,
            pom_mip_aware: self.pbr_enabled && self.pom_enabled && PBR_PROFILE_POM_MIP_AWARE,
            pom_adaptive_steps: self.pbr_enabled
                && self.pom_enabled
                && PBR_PROFILE_POM_ADAPTIVE_STEPS,
            pbr_companion_sampler: self.pbr_enabled && PBR_PROFILE_COMPANION_SAMPLER_TRILINEAR,
            pbr_vertex_lightgrid: self.pbr_enabled && PBR_PROFILE_VERTEX_LIGHTGRID,
            point_lights,
            map_light_simulation,
            source_map_world: source_map,
            legacy_dlights,
            vertex_dlights,
            clustered_lite_dlights,
            area_lights: self.emissive_area_lights_enabled,
            irradiance_volume: self.irradiance_volume_enabled,
            voxel_gi: self.voxel_probe_gi_enabled,
            // A local-shadow shader path is useless without a direct local-light
            // source. Normalize it out of the key so toggling shadows alone does
            // not create a redundant pipeline permutation.
            local_shadows: (self.local_light_shadows_enabled
                || self.dynamic_lights_mode == DynamicLightsMode::RayTracedHardware)
                && shadowable_local_lights,
            cascaded_shadows: self.cascaded_shadows_enabled,
            ray_traced_shadows: self.hardware_rt_requested() && self.ray_tracing_supported,
            ray_traced_sun: self.cascaded_shadow_mode == DynamicShadowsMode::RayTraced
                && self.ray_tracing_supported,
            planar_reflections: self.planar_reflection.active,
            // MAP/DEFAULT is a no-op on maps with no authored fog. FogSystem owns
            // the current map's authored-fog state and decides whether the legacy
            // JKA fragment path is actually required.
            legacy_fog: self.weather.fog.legacy_effective(),
            // Only while SP physics with the helper is engaged; the tint itself is
            // further gated by the camera uniform so jumps never respecialize.
            jump_shade: self.jump_shade.engaged(),
            ocean: self.ocean_enabled,
            weather_surface: self.weather_surface_needed(),
            classic_render_flags: self.classic_world_render_flags() != 0,
            static_bsp_ao: self.static_bsp_ao_enabled,
            planar_debug: self.planar_reflection_debug_mode != PlanarReflectionDebugMode::Off,
            deluxe: self.deluxe_shader_enabled(),
            deluxe_specular: self.deluxe_shader_enabled() && self.deluxe_specular > 0.0,
            detail_texture_mode: self.detail_textures_mode.shader_mode(),
        }
    }

    /// Deluxe needs the PBR material path; the shader override is what removes its
    /// cost, so this must match the key, not just the `pbr_settings.deluxe` uniform.
    pub(in crate::renderer) fn deluxe_shader_enabled(&self) -> bool {
        self.pbr_enabled && PBR_PROFILE_MATERIALS && self.deluxe_mapping_enabled
    }

    /// True while anything could shade wetness or puddles. Thresholds match the
    /// shader's own early-out (0.001); rain being enabled counts so the variant is
    /// compiled when the setting changes rather than mid-downpour.
    pub(in crate::renderer) fn weather_surface_needed(&self) -> bool {
        let rain = &self.weather.rain;
        rain.enabled
            || rain.surface_wetness > 0.001
            || rain.puddle_amount > 0.001
            || rain.puddle_debug_visualization
    }

    /// Keeps the active world variant in step with the flags whose source can change
    /// without a pipeline-aware setter: wetness accumulating or drying out, classic
    /// lighting toggles, cached-AO availability and planar debug views. One compare
    /// of a few bools per frame; the pipeline is only touched when one flips.
    pub(in crate::renderer) fn sync_runtime_variant_flags(&mut self) {
        let needed = self.weather_surface_needed();
        let classic = self.classic_world_render_flags() != 0;
        let static_ao = self.static_bsp_ao_enabled;
        let planar_debug = self.planar_reflection_debug_mode != PlanarReflectionDebugMode::Off;
        if self.world_variant_pending.is_some()
            || self.world.as_ref().is_some_and(|world| {
                let key = &world.active_pipeline_variant;
                key.weather_surface != needed
                    || key.classic_render_flags != classic
                    || key.static_bsp_ao != static_ao
                    || key.planar_debug != planar_debug
            })
        {
            self.activate_world_pipeline_variant();
        }
    }

    pub(in crate::renderer) fn enhanced_world_shader_needed(&self) -> bool {
        self.world_shader_variant_key().family() == WorldShaderFamily::Enhanced
    }

    pub(in crate::renderer) fn begin_settings_batch(&mut self) {
        self.settings_batch_open = true;
    }

    pub(in crate::renderer) fn end_settings_batch(&mut self) {
        self.settings_batch_open = false;
        if std::mem::take(&mut self.variant_activation_pending) {
            self.activate_world_pipeline_variant();
        }
    }

    pub(in crate::renderer) fn activate_world_pipeline_variant(&mut self) {
        if self.settings_batch_open {
            self.variant_activation_pending = true;
            return;
        }
        let mut key = self.world_shader_variant_key();
        if key.ray_traced_shadows && !self.ensure_ray_traced_shadow_resources() {
            // Preserve a valid raster pipeline if the adapter exposes the feature
            // but this particular map exceeds an acceleration-structure limit.
            key.ray_traced_shadows = false;
            key.ray_traced_sun = false;
        }
        let Some(current_key) = self
            .world
            .as_ref()
            .map(|world| world.active_pipeline_variant)
        else {
            self.world_variant_pending = None;
            return;
        };
        if current_key == key {
            self.world_variant_pending = None;
            return;
        }

        if self
            .world
            .as_ref()
            .is_some_and(|world| world.pipeline_variants.contains_key(&key))
        {
            if let Some(world) = &mut self.world {
                world.active_pipeline_variant = key;
                self.world_variant_pending = None;
                rverbose!(
                    3,
                    "Renderer pipeline cache: hit {} ({} cached variant(s))",
                    key.short_label(),
                    world.pipeline_variants.len()
                );
            }
            return;
        }

        let scene_format = self.scene_format();
        let samples = self.msaa_samples;
        let (layout, shader) = if key.ray_traced_shadows {
            let Some(rt) = self.ray_traced_shadows.as_ref() else {
                return;
            };
            match key.family() {
                WorldShaderFamily::Enhanced => {
                    (rt.world_pipeline_layout.clone(), rt.world_shader.clone())
                }
                WorldShaderFamily::Lean => (
                    rt.world_pipeline_layout_lean.clone(),
                    rt.world_shader_lean.clone(),
                ),
            }
        } else {
            match key.family() {
                WorldShaderFamily::Enhanced => (
                    self.world_pipeline_layout.clone(),
                    self.world_shader.clone(),
                ),
                WorldShaderFamily::Lean => (
                    self.world_pipeline_layout_lean.clone(),
                    self.world_shader_lean.clone(),
                ),
            }
        };
        let Some(plan) = self
            .world
            .as_ref()
            .map(WorldPipelineCompilePlan::from_world)
        else {
            return;
        };
        let job_key = PipelineJobKey::new(
            "world-variant",
            0,
            pipeline_hash(&(key, scene_format, samples, &plan)),
        );
        if let Some(variant) = self
            .pipeline_jobs
            .take_ready::<WorldPipelineVariant>(job_key)
        {
            if let Some(world) = &mut self.world {
                world.pipeline_variants.insert(key, variant);
                world.active_pipeline_variant = key;
                self.world_variant_pending = None;
                rverbose!(
                    3,
                    "Renderer pipeline cache: ready {} ({} cached variant(s))",
                    key.short_label(),
                    world.pipeline_variants.len(),
                );
            }
            return;
        }

        let device = self.device.clone();
        let queued =
            self.pipeline_jobs
                .request(job_key, "specialized BSP world variant", move || {
                    create_world_pipeline_variant_from_plan(
                        &device,
                        &layout,
                        &shader,
                        scene_format,
                        samples,
                        key,
                        &plan,
                    )
                });
        if queued {
            rverbose!(
                3,
                "Renderer pipeline cache: miss {} -> queued background compile",
                key.short_label()
            );
        }
        // Keep the previous fully valid variant live until the requested one is
        // complete. sync_runtime_variant_flags polls this pending target each frame.
        self.world_variant_pending = Some(key);
    }

    pub(in crate::renderer) fn set_planar_reflection_debug(
        &mut self,
        mode: PlanarReflectionDebugMode,
    ) {
        if self.planar_reflection_debug_mode == mode {
            return;
        }
        self.planar_reflection_debug_mode = mode;
        self.rebuild_frame_plan();
        self.last_planar_debug_selection = None;
        println!("Planar reflection debug: {}", mode.label());
    }

    pub(in crate::renderer) fn set_puddle_debug(&mut self, enabled: bool) {
        if self.weather.rain.puddle_debug_visualization == enabled {
            return;
        }
        self.weather.rain.puddle_debug_visualization = enabled;
        self.update_weather_surface_uniform();
        self.history_valid = false;
        self.ssr_history_valid = false;
        self.rebuild_frame_plan();
    }

    pub(in crate::renderer) fn set_entity_markers(
        &mut self,
        mesh: Option<jka_assets::bsp::DebugVolumeMesh>,
    ) {
        self.debug_volumes
            .set_entity_markers(&self.device, mesh.as_ref());
    }

    pub(in crate::renderer) fn set_force_unified_world(&mut self, force: bool) {
        if self.force_unified_world == force {
            return;
        }
        self.force_unified_world = force;
        self.rebuild_frame_plan();
    }

    pub(in crate::renderer) fn set_cull_debug(&mut self, mode: CullDebugMode) {
        if self.cull_debug_mode == mode {
            return;
        }
        self.cull_debug_mode = mode;
        if let Some(world) = &mut self.world {
            // Force the CPU PVS classification to be republished into the
            // debug-reason buffer when diagnostics are switched on.
            world.active_selection_key = None;
        }
        if mode == CullDebugMode::Off {
            self.cull_diagnostics_readback.latest = [0; 4];
            self.cull_diagnostics_readback.current = None;
            self.cull_diagnostics_readback.encoded = false;
        }
        self.rebuild_frame_plan();
        self.update_gpu_cull_settings();
    }
}
