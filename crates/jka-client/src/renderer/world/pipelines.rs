//! World pipelines.
use crate::renderer::{
    camera_depth_compare, create_cull_debug_pipeline, create_ssr_visibility_resources,
    create_surface_inspector_pipeline, create_taa_post_pipeline, create_targets,
    create_world_pipeline, gpu_blend_factor, pipeline_hash, scene, thread, AlphaGen, BTreeMap, BTreeSet, BlendFactor,
    BlendMode, CullDebugMode, CullMode, DrawBatch, DrawClass, GpuVertex, Instant, MaterialFragmentKey,
    MaterialUniform,
    OnceLock, Ordering, PipelineJobKey, PipelineKey, Renderer, RgbGen, TcGen, TcMod, Vec3,
    WorldBatch, WorldPipelineCompilePlan, WorldPipelineVariant, WorldShaderFamily,
    WorldShaderVariantKey, DEPTH_FORMAT, SHADOW_CASTER_CULL, VERTEX_ATTRIBUTES,
};

impl Renderer {
    pub(in crate::renderer) fn native_atmosphere_active(&self) -> bool {
        self.sky_backend == 1 || (self.cloud_backend == 1 && self.clouds_enabled)
    }

    pub(in crate::renderer) fn native_display_mapping_active(&self) -> bool {
        // The meter reads the Blue Water sky's environment map, so it is only
        // meaningful when that sky is the one on screen (native clouds alone
        // leave the JKA skybox visible).
        self.sky_backend == 1 && self.blue_water_sky.is_some() && self.blue_water_environment.filmic
    }

    pub(in crate::renderer) fn effective_auto_exposure_strength(&self) -> f32 {
        if self.native_display_mapping_active() {
            // Native display mode owns sky-referenced eye adaptation and AgX.
            1.0
        } else if self.hdr_enabled && self.tone_mapping_enabled && self.auto_exposure_enabled {
            self.auto_exposure_strength
        } else {
            0.0
        }
    }
    /// Format the gameplay scene renders into, ignoring the asset viewer.
    pub(in crate::renderer) fn gameplay_scene_format(&self) -> wgpu::TextureFormat {
        // Legacy OpenGL starts with GL_DITHER enabled, and the OpenJK/TaystJK
        // vanilla renderer path leaves that default in place. WGPU has no
        // equivalent fixed-function framebuffer dither, so r_drawfog 2 otherwise
        // quantizes its smooth EXP2
        // blend directly into our 8-bit sRGB scene target and produces obvious
        // contour bands. Keep only Legacy 2 on a float scene target, then do the
        // single 8-bit quantization in the presentation pass where we can dither
        // it explicitly. This is deliberately narrower than r_hdr: it changes
        // precision/order only, not tone mapping or any HDR lighting semantics.
        let legacy2_precision = self.weather.fog.legacy2_precision_active();
        // Native atmosphere and weather produce HDR radiance even when the
        // host's HDR option is off. Clamping it in an 8-bit scene destroys the
        // contrast between the lightning channel and its surrounding airlight.
        if self.hdr_enabled
            || self.native_atmosphere_active()
            || self.blue_water_environment.weather_backend == 1
            || self.glow_mode.enabled()
            || self.halation_enabled
            || legacy2_precision
        {
            wgpu::TextureFormat::Rgba16Float
        } else {
            self.config.format
        }
    }

    /// The asset viewer draws without a gameplay post pass. Its color PSOs and
    /// MSAA resolve must match the actual preview destination: SDR swapchain
    /// or, when native HDR is active, the Rgba16Float HDR compositor input.
    /// Gameplay HDR/bloom settings alone must not force an FP16 SDR preview.
    pub(in crate::renderer) fn preview_scene_format(&self) -> wgpu::TextureFormat {
        if self.hdr_output.is_some() {
            wgpu::TextureFormat::Rgba16Float
        } else {
            self.config.format
        }
    }

    pub(in crate::renderer) fn scene_format(&self) -> wgpu::TextureFormat {
        if self.asset_preview_mode {
            self.preview_scene_format()
        } else {
            self.gameplay_scene_format()
        }
    }

    pub(in crate::renderer) fn set_asset_preview_mode(&mut self, enabled: bool) {
        let old_scene_format = self.scene_format();
        self.asset_preview_mode = enabled;
        if !enabled {
            self.asset_preview_viewport = None;
        }
        if old_scene_format != self.scene_format() {
            self.request_scene_rebuild();
        }
    }

    pub(in crate::renderer) fn rebuild_world_pipelines(&mut self) {
        // A genuine format/MSAA transition needs compatible PSOs immediately.
        // Compile the raster fallback synchronously (normally a few ms), NEVER
        // the multi-second RT specialization on the render thread. The desired
        // RT variant is queued with activate_world_pipeline_variant() below, and
        // becomes active only after its background job has completed.
        let started = Instant::now();
        let mut key = self.world_shader_variant_key();
        let rt_requested = key.ray_traced_shadows;
        if rt_requested {
            key.ray_traced_shadows = false;

            key.rt_receiver_glass = false;
        }
        let scene_format = self.scene_format();
        let samples = self.msaa_samples;
        let (layout, shader) = if key.ray_traced_shadows {
            let Some(rt) = self.ray_traced_shadows.as_ref() else {
                return;
            };
            match key.family() {
                WorldShaderFamily::Enhanced => (&rt.world_pipeline_layout, rt.world_shader_for_variant(key)),
                WorldShaderFamily::Lean => (&rt.world_pipeline_layout_lean, rt.world_shader_for_variant(key)),
            }
        } else {
            match key.family() {
                WorldShaderFamily::Enhanced => (&self.world_pipeline_layout, &self.world_shader),
                WorldShaderFamily::Lean => {
                    (&self.world_pipeline_layout_lean, &self.world_shader_lean)
                }
            }
        };
        let (
            pipelines,
            legacy_dlight_pipelines,
            fog_pass_pipelines,
            reflection_pipelines,
            reflection_fog_pass_pipelines,
            specialized_pipelines,
        ) = {
            let Some(world) = self.world.as_ref() else {
                return;
            };
            let pipelines = create_world_pipelines(
                &self.device,
                layout,
                shader,
                scene_format,
                samples,
                &world.coarse_batches,
                key.legacy_fog,
                Some(key),
            );
            let legacy_dlight_pipelines = if key.legacy_dlights {
                create_legacy_dlight_pass_pipelines(
                    &self.device,
                    layout,
                    shader,
                    scene_format,
                    samples,
                    &world.coarse_batches,
                    key,
                )
            } else {
                BTreeMap::new()
            };
            let fog_pass_pipelines = if key.legacy_fog {
                create_legacy_fog_pass_pipelines(
                    &self.device,
                    layout,
                    shader,
                    scene_format,
                    samples,
                    &world.coarse_batches,
                    Some(key),
                )
            } else {
                BTreeMap::new()
            };
            let reflection_pipelines =
                if world.planar_reflectors.is_empty() || !key.planar_reflections {
                    BTreeMap::new()
                } else {
                    create_reflection_pipelines(
                        &self.device,
                        layout,
                        shader,
                        scene_format,
                        &world.coarse_batches,
                        key.legacy_fog,
                        key,
                    )
                };
            let reflection_fog_pass_pipelines = if key.legacy_fog
                && !world.planar_reflectors.is_empty()
                && key.planar_reflections
            {
                create_reflection_fog_pass_pipelines(
                    &self.device,
                    layout,
                    shader,
                    scene_format,
                    &world.coarse_batches,
                    key,
                )
            } else {
                BTreeMap::new()
            };
            let specialized_pipelines = create_specialized_world_pipelines(
                &self.device,
                layout,
                shader,
                &self.world_shader_lean,
                scene_format,
                samples,
                &material_specialization_jobs(world.all_world_batches()),
                key,
            );
            (
                pipelines,
                legacy_dlight_pipelines,
                fog_pass_pipelines,
                reflection_pipelines,
                reflection_fog_pass_pipelines,
                specialized_pipelines,
            )
        };

        self.world_variant_pending = None;
        if let Some(world) = &mut self.world {
            world.pipeline_variants.clear();
            world.pipeline_variants.insert(
                key,
                WorldPipelineVariant {
                    ocean_far_pipeline: None,
                    ocean_native_mesh_pipeline: None,
                    ocean_native_mesh_wireframe_pipeline: None,
                    pipelines,
                    legacy_dlight_pipelines,
                    fog_pass_pipelines,
                    reflection_pipelines,
                    reflection_fog_pass_pipelines,
                    specialized_pipelines,
                    slot_pipelines: Vec::new(),
                },
            );
            world.active_pipeline_variant = key;
            world.refresh_pso_slots();
            // MSAA/scene-format changed: drop the fast set, it is recompiled on
            // demand the next time the frame plan selects FastBaseline.
            world.fast_pipelines.clear();
            world.fast_complexity_pipelines.clear();
            world.complexity_pipelines.clear();
        }
        rverbose!(1, "Synchronous world format/MSAA fallback compiled in {:.1} ms (RT requested: {rt_requested})",
            started.elapsed().as_secs_f64() * 1000.0);
        if rt_requested {
            self.activate_world_pipeline_variant();
        }
    }

    /// The known-fast baseline pipelines only serve `WorldRenderPath::FastBaseline`.
    /// They are compiled the first frame that path runs instead of alongside every
    /// advanced variant at map load and on every scene rebuild.
    pub(in crate::renderer) fn ensure_fast_world_pipelines(&mut self) -> bool {
        let scene_format = self.scene_format();
        let samples = self.msaa_samples;
        let Some(world) = self.world.as_mut() else {
            return false;
        };
        if !world.fast_pipelines.is_empty() {
            return true;
        }
        let unique_keys = world
            .coarse_batches
            .iter()
            .map(|batch| batch.source.pipeline)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let key = PipelineJobKey::new(
            "fast-world",
            0,
            pipeline_hash(&(scene_format, samples, &unique_keys)),
        );
        if let Some(pipelines) = self
            .pipeline_jobs
            .take_ready::<BTreeMap<PipelineKey, wgpu::RenderPipeline>>(key)
        {
            world.fast_pipelines = pipelines;
            return true;
        }
        let device = self.device.clone();
        let layout = self.fast_world_pipeline_layout.clone();
        let shader = self.fast_world_shader.clone();
        let batches = world.coarse_batches.clone();
        self.pipeline_jobs
            .request(key, "fast-baseline world set", move || {
                create_world_pipelines(
                    &device,
                    &layout,
                    &shader,
                    scene_format,
                    samples,
                    &batches,
                    false,
                    None,
                )
            });
        false
    }

    /// Queue the scene rebuild instead of running it inline.
    ///
    /// Several independent settings request target-resource rebuilds — bloom,
    /// SSAO, SSR, depth of field, cloud resolution and MSAA. Collapse bursts of
    /// changes into one target refresh. World PSOs are invalidated only for
    /// actual format/MSAA changes; slow RT variants remain asynchronous.
    pub(in crate::renderer) fn request_scene_rebuild(&mut self) {
        self.scene_rebuild_pending = true;
    }

    pub(in crate::renderer) fn apply_pending_scene_rebuild(&mut self) {
        if std::mem::take(&mut self.scene_rebuild_pending) {
            self.rebuild_scene_targets_and_pipelines();
        }
    }

    pub(in crate::renderer) fn rebuild_scene_targets_and_pipelines(&mut self) {
        let scene_format = self.scene_format();
        let samples = self.msaa_samples;
        // Post-resource changes (bloom, SSAO, SSR, DOF, cloud resolution) do NOT
        // invalidate render pipelines that still target the same format/MSAA.
        // In particular, avoid recompiling expensive RT BSP variants here.
        // A resize / surface reconfigure may have recreated `targets` using
        // the new HDR scene format *before* the pending rebuild is processed.
        // Therefore target texture compatibility alone cannot prove that the
        // BSP, Ghoul2, MD3, grass and weather PSOs also match. Compare against
        // the last completed PIPELINE rebuild, not just the current texture.
        let target_compat_changed = self.targets._scene_texture.format() != scene_format
            || self.targets._msaa_texture.as_ref().map_or(1, |texture| texture.sample_count()) != samples;
        let pipeline_compat_changed = target_compat_changed
            || self.scene_pipeline_compat != (scene_format, samples)
            || self.dynamic_model_renderer.fx_sprite_surface_format != scene_format
            || self.dynamic_model_renderer.fx_sprite_samples != samples;
        if crate::pipeline_jobs::pipeline_trace_enabled() {
            println!("[scene rebuild] format={scene_format:?} samples={samples} target_changed={target_compat_changed} last_pipeline_compat={:?} model_format={:?} model_samples={} rebuild_pipelines={pipeline_compat_changed}",
                self.scene_pipeline_compat,
                self.dynamic_model_renderer.fx_sprite_surface_format,
                self.dynamic_model_renderer.fx_sprite_samples);
        }
        // Companion scene textures/pipelines share the gameplay scene format and
        // MSAA count. Recreate them lazily after this rebuild; the presentation
        // worker keeps showing its last safe texture until the new ring arrives.
        self.companion_scene_targets.clear();
        self.targets = create_targets(
            &self.device,
            self.config.width,
            self.config.height,
            scene_format,
            samples,
            self.glow_mode.enabled() || self.halation_enabled,
            self.glow_mode.authored(),
            self.depth_of_field_strength > 0.001,
            self.ssao_enabled,
            self.ssr_enabled,
            self.needs_resolved_depth_target(),
            self.taa_enabled || self.rt_taa_mode > 0,
            self.clouds_enabled && self.cloud_backend == 0,
            self.cloud_render_resolution,
        );
        self.world_flares.invalidate_depth_binding();
        self.world_flares_sky.invalidate_depth_binding();
        let ssr_visibility_resources =
            create_ssr_visibility_resources(&self.device, &self.targets, samples);
        let (ssr_visibility_layout, ssr_visibility_pipeline, ssr_visibility_bind_group) =
            ssr_visibility_resources
                .map_or((None, None, None), |(layout, pipeline, bind_group)| {
                    (Some(layout), Some(pipeline), Some(bind_group))
                });
        self._ssr_visibility_layout = ssr_visibility_layout;
        self.ssr_visibility_pipeline = ssr_visibility_pipeline;
        self.ssr_visibility_bind_group = ssr_visibility_bind_group;
        if pipeline_compat_changed {
            self.weather
                .rain
                .rebuild_render_pipelines(&self.device, scene_format, samples);
            self.weather
                .snow
                .rebuild_render_pipeline(scene_format, samples);
            self.weather
                .atmosphere
                .rebuild_render_pipeline(scene_format, samples);
            self.weather
                .wind_visuals
                .rebuild_render_pipelines(scene_format, samples);
            self.grass_renderer
                .rebuild_pipeline(&self.device, scene_format, samples);
            self.surface_sprite_effect_renderer
                .rebuild_pipelines(&self.device, scene_format, samples);
            self.dynamic_model_renderer
                .rebuild_pipelines(&self.device, scene_format, samples);
            self.dynamic_model_renderer
                .prewarm_core_model_pipelines(&mut self.pipeline_jobs, &self.device);
        }
        // One shared spray pipeline serves every ocean; recompiled on demand by
        // ensure_lazy_scene_pipelines only while some ocean has sea spray on.
        self.ocean_spray_pipeline = None;
        self.rebuild_planar_reflection_resources();
        // TAA resolve and diagnostic pipelines depend on the scene target
        // format/MSAA; drop them and let ensure_lazy_scene_pipelines recompile
        // only the ones the current settings actually use.
        self.taa_post_pipeline = None;
        self.thermal_taa_pipeline = None;
        self.rebuild_post_bind_group();
        self.rebuild_rain_haze_mask_bind_group();
        self.rebuild_gpu_visibility_resources();
        self.history_valid = false;
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
        self.update_post_uniform();
        self.update_lighting_settings();
        if pipeline_compat_changed {
            // True format/MSAA changes still need compatible PSOs. These are
            // relatively rare; a transactional async format switch is separate
            // work because old targets must remain valid until all PSOs are ready.
            self.rebuild_world_pipelines();
            self.cloud_shadow_opaque_pipeline = None;
            self.cull_debug_pipeline = None;
            self.wireframe_pipeline = Default::default();
            self.world_wireframe_pipelines.clear();
            self.fast_world_wireframe_pipeline = None;
            self.debug_volumes.clear_pipelines();
            self.surface_inspector_pipeline = None;
            // Commit compatibility only after every format-sensitive pipeline
            // cache has been invalidated/rebuilt. Async variants include their
            // format in the job key and are installed when ready.
            self.scene_pipeline_compat = (scene_format, samples);
        } else {
            // In a resource-only rebuild, keep the old GPU pipelines cached,
            // but pick up any shader-key change (e.g. legacy fog) asynchronously.
            self.activate_world_pipeline_variant();
        }
    }

    /// Compiles the pipelines that are only needed while TAA, the surface
    /// inspector, or the cull-rejection overlay are active. Runs on the cold
    /// pre-frame path so the frame recorder can borrow them immutably.
    pub(in crate::renderer) fn ensure_lazy_scene_pipelines(&mut self) {
        self.ensure_rt_taa_pipeline();
        let scene_format = self.scene_format();
        let samples = self.msaa_samples;

        if self.taa_enabled && self.taa_post_pipeline.is_none() {
            let key = PipelineJobKey::new(
                "taa-post",
                0,
                pipeline_hash(&(self.config.format, scene_format)),
            );
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.taa_post_pipeline = Some(pipeline);
            } else {
                let device = self.device.clone();
                let layout = self.post_pipeline_layout.clone();
                let shader = self.post_shader.clone();
                let output_format = self.config.format;
                self.pipeline_jobs.request(key, "TAA post", move || {
                    create_taa_post_pipeline(&device, &layout, &shader, output_format, scene_format)
                });
            }
        }

        if self.inspector_vertex_range.is_some() && self.surface_inspector_pipeline.is_none() {
            let key = PipelineJobKey::new(
                "surface-inspector",
                0,
                pipeline_hash(&(scene_format, samples)),
            );
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.surface_inspector_pipeline = Some(pipeline);
            } else {
                let device = self.device.clone();
                let layout = self.wireframe_pipeline_layout.clone();
                let shader = self.surface_inspector_shader.clone();
                self.pipeline_jobs
                    .request(key, "surface inspector", move || {
                        create_surface_inspector_pipeline(
                            &device,
                            &layout,
                            &shader,
                            scene_format,
                            samples,
                        )
                    });
            }
        }

        if self.ocean_spray_pipeline.is_none()
            && (self
                .ocean
                .as_ref()
                .is_some_and(|ocean| ocean.spray_active())
                || self
                    .authored_oceans
                    .iter()
                    .any(|(_, ocean)| ocean.spray_active()))
        {
            let key =
                PipelineJobKey::new("ocean-spray", 0, pipeline_hash(&(scene_format, samples)));
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.ocean_spray_pipeline = Some(pipeline);
            } else {
                let device = self.device.clone();
                let camera_layout = self.camera_layout.clone();
                let ocean_layout = self.ocean_layout.clone();
                self.pipeline_jobs.request(key, "ocean spray", move || {
                    crate::ocean::create_spray_pipeline(
                        &device,
                        &camera_layout,
                        &ocean_layout,
                        scene_format,
                        samples,
                    )
                });
            }
        }

        if self.cull_debug_mode.draws_overlay()
            && self.cull_debug_pipeline.is_none()
        {
            let hiz_only = self.cull_debug_mode == CullDebugMode::HiZOnly;
            let key = PipelineJobKey::new("cull-debug", 0, pipeline_hash(&(scene_format, samples, hiz_only)));
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.cull_debug_pipeline = Some(pipeline);
            } else {
                let device = self.device.clone();
                let layout = self.cull_debug_pipeline_layout.clone();
                let shader = self.cull_debug_shader.clone();
                self.pipeline_jobs
                    .request(key, "cull rejection debug", move || {
                        create_cull_debug_pipeline(&device, &layout, &shader, scene_format, samples, hiz_only)
                    });
            }
        }
    }
}

pub(in crate::renderer) fn material_uniform(
    source: &DrawBatch,
    detail_texture_eligible: bool,
) -> MaterialUniform {
    let mut out = MaterialUniform::default();
    match source.tc_gen {
        TcGen::Base => out.header[0] = 0,
        TcGen::Lightmap => out.header[0] = 1,
        TcGen::Vector(s, t) => {
            out.header[0] = 2;
            // tcGen vectors are authored in JKA coordinates. Vertex positions in
            // the renderer use [x,z,-y], so transform the basis once here.
            out.vector_s = [s[0], s[2], -s[1], 0.0];
            out.vector_t = [t[0], t[2], -t[1], 0.0];
        }
        TcGen::Environment => out.header[0] = 3,
        TcGen::SkyCloud(height) => {
            // Per-pixel view-direction projection onto the cloud layer; the
            // sky fragment shader consumes the height from params[2].
            out.header[0] = 4;
            out.params[2] = height;
        }
    }
    let count = source.tc_mods.len().min(4);
    out.header[1] = count as u32;
    // Keep RGB and alpha generation independent, matching id Tech 3's
    // separate rgbGen/alphaGen state. Bit 0 remains the legacy "RGB from
    // vertex color" flag so existing shader paths keep their compact layout.
    if matches!(source.rgb_gen, RgbGen::Vertex | RgbGen::ExactVertex) {
        out.header[2] |= 1;
    } else if matches!(source.rgb_gen, RgbGen::OneMinusVertex) {
        out.header[2] |= 524288;
    }

    // In the classic renderer rgbGen vertex leaves vertex alpha in place when
    // alphaGen is identity (with the normal identity-light scale). exactVertex
    // does not. Encode that implicit case here so the WGSL need not know the
    // parser's defaulting rules.
    let alpha_from_vertex = matches!(source.alpha_gen, AlphaGen::Vertex)
        || (matches!(source.alpha_gen, AlphaGen::Identity)
            && matches!(source.rgb_gen, RgbGen::Vertex));
    if alpha_from_vertex {
        out.header[2] |= 1048576;
    } else if matches!(source.alpha_gen, AlphaGen::OneMinusVertex) {
        out.header[2] |= 2097152;
    }

    // Waveform generators replace the stage's RGB / alpha in the shader.
    if let RgbGen::Wave(wave) = source.rgb_gen {
        out.header[2] |= 134217728;
        out.wave_rgb = [wave.base, wave.amplitude, wave.phase, wave.frequency];
        out.wave_funcs[0] = wave.func.id();
    }
    if let AlphaGen::Wave(wave) = source.alpha_gen {
        out.header[2] |= 268435456;
        out.wave_alpha = [wave.base, wave.amplitude, wave.phase, wave.frequency];
        out.wave_funcs[1] = wave.func.id();
    }

    if source.modulate_lightmap {
        out.header[2] |= 2;
    }
    if source.texture_is_lightmap {
        out.header[2] |= 8388608;
    }
    if source.lightmap.is_some() {
        out.header[2] |= 16777216;
    }
    if matches!(source.pipeline.blend, BlendMode::Opaque) {
        out.header[2] |= 33554432;
    }
    // One legacy RGB policy for the original fused overlay and the additive
    // source approximation. The exact fusion uses its existing opaque shader
    // entry point; only the additive mode needs a fragment-uniform bit.
    // No new pipeline variant, render pass, or draw is introduced here.
    if matches!(source.legacy_rgb_compositing(),
        scene::LegacyRgbCompositing::EncodedAdditiveSource)
    {
        out.header[2] |= 536870912; // MATERIAL_LEGACY_ADDITIVE_SRGB
    }
    // Depth prepass packs this into the reflection-policy attachment so the
    // final Legacy 1 display-space fog affects exactly the BSP surfaces that
    // participate in the compiled/recovered global fog (q3map_nofog stays out).
    if source.fog_is_global && source.fog[3] > 0.001 && source.global_fog_post_eligible {
        out.header[2] |= 67108864;
    }
    // Dedicated cached-AO receiver bit for q3map LIGHTMAP_BY_VERTEX stages.
    // Restrict it to the stage that actually consumes BSP vertex RGB so
    // emissive/additive sibling stages do not get incorrectly darkened.
    if source.vertex_lit && source.rgb_gen.uses_vertex_color() {
        out.header[2] |= 131072;
    }
    let pbr_eligible = !source.texture_is_lightmap
        && !source.texture_is_white
        && source.pipeline.class == DrawClass::Opaque
        && source.pipeline.blend == BlendMode::Opaque
        && matches!(source.tc_gen, TcGen::Base);
    if pbr_eligible && source.normal_texture.is_some() {
        out.header[2] |= 8;
    }
    if pbr_eligible && source.roughness_texture.is_some() {
        out.header[2] |= 16;
    }
    if pbr_eligible && source.height_texture.is_some() {
        out.header[2] |= 32;
    }
    if pbr_eligible && source.metallic_texture.is_some() {
        out.header[2] |= 64;
    }
    if pbr_eligible && source.specular_texture.is_some() {
        out.header[2] |= 128;
    }
    if pbr_eligible && source.emissive_texture.is_some() {
        out.header[2] |= 256;
    }
    if pbr_eligible && source.rmo_packed {
        out.header[2] |= 512;
    }
    if pbr_eligible && source.rmo_specular_alpha {
        out.header[2] |= 262144;
    }
    if pbr_eligible && source.height_from_alpha {
        out.header[2] |= 1024;
    }
    if source.water && source.authored_ocean.is_none() {
        out.ocean_waves = [1.0, source.water_amplitude, 0.0, 0.0];
    }
    if source.water_primary {
        // Enhanced-water promotion: the authored first stage becomes the single
        // GodotOceanWaves surface pass while sibling authored stages are skipped.
        out.header[2] |= 65536;
    }
    for (i, modifier) in source.tc_mods.iter().take(4).enumerate() {
        let a = i * 2;
        match *modifier {
            TcMod::Scroll(x, y) => out.mods[a] = [1.0, x, y, 0.0],
            TcMod::Scale(x, y) => out.mods[a] = [2.0, x, y, 0.0],
            TcMod::Rotate(degrees) => out.mods[a] = [3.0, degrees, 0.0, 0.0],
            TcMod::Transform(v) => {
                // `tcMod transform m00 m01 m10 m11 t0 t1` is applied by the
                // engine as s' = s*m00 + t*m10 + t0, t' = s*m01 + t*m11 + t1,
                // so the shader's first row is (m00, m10) and second (m01, m11).
                out.mods[a] = [4.0, v[0], v[2], v[4]];
                out.mods[a + 1] = [v[1], v[3], v[5], 0.0];
            }
            TcMod::Stretch(wave) => {
                // Store the authored waveform in the existing two-vec4 mod slot.
                out.mods[a] = [6.0, wave.base, wave.amplitude, wave.phase];
                out.mods[a + 1] = [wave.frequency, wave.func.id() as f32, 0.0, 0.0];
            }
            TcMod::Turb {
                base,
                amplitude,
                phase,
                frequency,
            } => {
                out.mods[a] = [5.0, base, amplitude, phase];
                out.mods[a + 1] = [frequency, 0.0, 0.0, 0.0];
            }
        }
    }
    if !source.texture_is_lightmap
        && source.pipeline.class != DrawClass::Sky
        && source.pipeline.blend == BlendMode::Opaque
    {
        out.header[2] |= 4;
    }
    if source.skybox.is_some() {
        out.header[3] |= 1;
    }
    if source.dlight_in_lightmap_stage {
        out.header[3] |= 1 << 16;
    }
    if detail_texture_eligible {
        out.header[3] |= 2;
    }
    out.header[3] |= u32::from(source.surface_material & 31) << 8;
    // Thermal-specific classifier reads these low unused header.w bits only
    // when the lazy thermal prepass is active. Regular shaders ignore them.
    if source.thermal_lava { out.header[3] |= 1 << 3; }
    // Stage-level glow/additive emission is resolved by the authored thermal
    // capture at individual texels, not in the opaque depth prepass. Only an
    // explicit q3map_surfacelight has whole-surface heat (level 3).
    if source.thermal_emission == 3 { out.header[3] |= 1 << 5; }
    if source.foliage_wind {
        out.header[3] |= crate::foliage::MATERIAL_FLAG;
    }
    if source.glass_primary {
        out.header[3] |= 1 << 17;
        out.ocean_color = [source.glass_tint[0], source.glass_tint[1], source.glass_tint[2], source.glass_reflectance];
    }
    out.color = source.color;
    // TaystJK rd-vanilla/tr_shade_calc.cpp RB_CalcDiffuseColor evaluates
    // the CURRENT refEntity's ambient + max(dot(N,L),0)*directed lighting.
    // BSP world surfaces use tr.worldEntity, whose ambient/directed fields
    // are zero (unlike dynamic refEntities, which have lightgrid lighting).
    // Accordingly CGEN_LIGHTING_DIFFUSE on a world surface produces black
    // RGB while preserving the separately authored alpha. Do NOT substitute
    // BSP vertex color: that's the distinct CGEN_VERTEX/EXACT_VERTEX mode.
    // Models retain their own existing lightgrid/model-lighting path.
    if matches!(source.rgb_gen, RgbGen::LightingDiffuse | RgbGen::LightingDiffuseEntity) {
        out.color[..3].fill(0.0);
    }
    // The map loader can recover the specific GE128/127 depth-seed boundary
    // without changing the shader script or globally forcing BSP vertex alpha.
    if source.alpha_test_texture_only {
        out.header[2] |= 1073741824; // MATERIAL_MASK_TEXTURE_ALPHA_ONLY
    }
    // Opaque world pipelines are compiled with ENABLE_ALPHA_TEST off.
    debug_assert!(
        source.pipeline.class != DrawClass::Opaque || source.alpha_cutoff == 0.0,
        "Opaque batch with alpha cutoff {}",
        source.alpha_cutoff
    );
    out.params[0] = source.alpha_cutoff;
    out.params[1] = source.parallax_depth;
    if source.pipeline.class != DrawClass::Sky {
        // Relative *stage* complexity estimate, fixed at material preparation.
        // This isn't driver ISA instruction count or measured GPU time. Separate
        // stages produce separate diagnostic draws; fused additive stages get
        // an explicit multiplier because the renderer draws them together.
        let mut cost = 2.0f32;
        cost += source.tc_mods.len().min(4) as f32 * 0.35;
        if source.lightmap.is_some() || source.modulate_lightmap { cost += 1.2; }
        if source.alpha_cutoff != 0.0 { cost += 0.8; }
        if source.pipeline.blend != BlendMode::Opaque { cost += 2.5; }
        if source.normal_texture.is_some() { cost += 2.0; }
        if source.height_texture.is_some() { cost += 3.0; }
        if source.roughness_texture.is_some() { cost += 0.6; }
        if source.metallic_texture.is_some() { cost += 0.6; }
        if source.specular_texture.is_some() { cost += 0.6; }
        if source.emissive_texture.is_some() { cost += 0.6; }
        if source.reflection_probe.is_some() { cost += 1.5; }
        out.params[2] = cost * f32::from(source.pipeline.additive_fusion_count.max(1));
    }
    // Tayst/OpenJK R_RemapShader's timeOffset is subtracted from shader time.
    // params.w was intentionally unused by world materials, so carrying it
    // here does not grow MaterialUniform or alter bind-group layouts.
    out.params[3] = source.shader_time_offset;
    // tcGen vector uses xyz only; the reserved w lane also exists in depth/shadow/RT materials.
    out.vector_t[3] = source.shader_clamp_time.unwrap_or(0.0);
    out.pbr_params0 = [
        source.normal_scale[0],
        source.normal_scale[1],
        source.roughness_override.unwrap_or(-1.0),
        source.reflection_roughness_hint,
    ];
    if let Some(reflectance) = source.specular_reflectance {
        out.pbr_params1 = [reflectance[0], reflectance[1], reflectance[2], 1.0];
    }
    let has_pbr_companion = source.normal_texture.is_some()
        || source.roughness_texture.is_some()
        || source.metallic_texture.is_some()
        || source.specular_texture.is_some()
        || source.roughness_override.is_some()
        || source.specular_reflectance.is_some();
    if pbr_eligible && has_pbr_companion && source.reflection_probe.is_some() {
        out.header[2] |= 2048;
        out.reflection_probe = source.reflection_probe_position_radius;
    }
    if (source.reflection_cache_flags & scene::REFLECTION_CACHE_SSR) != 0 {
        out.header[2] |= 4194304;
    }
    // header.w bit 2 is free. header.z bit 23 already marks explicit lightmaps.
    // This authored-material guard requires no new bindings or pipeline variants.
    if source.authored_environment_composite {
        out.header[3] |= 1 << 2;
    }
    if source.planar_reflection {
        out.header[2] |= 4096;
        out.planar_plane = source.planar_plane;
    } else if source.planar_environment_candidate {
        out.header[2] |= 8192;
        // A blended tcGen-environment stage is not a mirror replacement in the
        // material sense: its legacy environment texture also authored the
        // strength/tint of the reflection contribution. Preserve that response
        // when substituting the live planar scene.
        if source.pipeline.blend != BlendMode::Opaque {
            out.header[2] |= 32768;
        }
        out.planar_plane = source.planar_plane;
    }
    let planar_normal = Vec3::new(
        source.planar_plane[0],
        source.planar_plane[1],
        source.planar_plane[2],
    );
    if planar_normal.length_squared() > 0.5 {
        // Debug-only bit. assign_planar_reflection_planes propagates the
        // resolved plane to sibling stages so diagnostics cannot be hidden by
        // a later alpha/lightmap pass of the same material.
        out.header[2] |= 16384;
        out.planar_plane = source.planar_plane;
    }
    out
}

/// Effective native identity of a BSP pass. Cache keys live at the native PSO
/// level, not at the menu/preset level. Shader-specialization constants remain
/// compile-time WGSL overrides; this is NOT a universal shader or runtime branch.
///
/// Each pass declares its dependency boundary here. Base rendering excludes
/// Legacy dlights because those execute in their own additional pass; fog and
/// reflection passes keep their distinct entry points and vertex formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::renderer) enum WorldPipelinePass {
    Base { authored_fog: bool },
    LegacyDlight,
    LegacyFog,
    Reflection { authored_fog: bool },
    ReflectionFog,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::renderer) struct WorldPassPipelineIdentity {
    pub variant: WorldShaderVariantKey,
    pub pass: WorldPipelinePass,
    pub native_key: PipelineKey,
    pub target_format: wgpu::TextureFormat,
    pub samples: u32,
    /// Narrowed material key of a specialized base-pass PSO; None for general ones.
    pub material: Option<MaterialFragmentKey>,
    /// The specialized PSO uses the minimal entry points (r_fragmentShaderMode 2).
    pub minimal: bool,
}

impl WorldPassPipelineIdentity {
    pub(in crate::renderer) fn new(
        variant: WorldShaderVariantKey, pass: WorldPipelinePass,
        native_key: PipelineKey, target_format: wgpu::TextureFormat, samples: u32,
    ) -> Self {
        // Per-pass normalization is explicit and centralized. Other features
        // must NEVER be stripped unless their pipeline code actually disables
        // them; keeping an extra permutation is safer than aliasing wrong code.
        let mut variant = match pass {
            WorldPipelinePass::Base { .. } => variant.base_pass_key(),
            _ => variant,
        };
        // The flags only add specialized PSOs; the general ones are the same program
        // in every mode, so switching r_fragmentShaderMode reuses them.
        variant.material_specialization = false;
        variant.minimal_entries = false;
        variant.program_mode = super::programs::WorldProgramMode::Specialized;
        Self { variant, pass, native_key, target_format, samples, material: None, minimal: false }
    }

    pub(in crate::renderer) fn specialized(
        variant: WorldShaderVariantKey, authored_fog: bool, native_key: PipelineKey,
        material: MaterialFragmentKey, minimal: bool, target_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        Self {
            material: Some(material),
            minimal,
            ..Self::new(super::programs::program_variant(variant, material), WorldPipelinePass::Base { authored_fog }, native_key,
                target_format, samples)
        }
    }
}

/// TaystJK skybox_pipeline: one PSO per outer-box face (rt, bk, lf, ft, up, dn),
/// each a single texture sample (`vs_sky_box_*` / `fs_sky_box_*`). Drawn after
/// the opaque world at far depth with depth writes off, so only uncovered pixels
/// are shaded.
/// Fullscreen analytic far ocean, compiled lazily on the existing pipeline
/// worker. It uses the active enhanced BSP module, so FFT/optics/materials
/// are identical to the normal ocean; it is NOT another renderer path.
pub(in crate::renderer) fn create_ocean_far_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    variant: WorldShaderVariantKey,
) -> wgpu::RenderPipeline {
    // The fullscreen vertex stage uses no shader overrides. Pass only
    // overrides reachable from fs_ocean_far (ocean_surface_sample and its
    // lighting/fog helpers). Supplying the fs_main-only set can cause a WGPU
    // validation error when this optional pipeline first compiles.
    let mut constants = variant.base_pass_key().compilation_constants();
    constants.retain(|(name, _)| matches!(*name,
        "ENABLE_PBR" | "ENABLE_POINT_LIGHTS" | "ENABLE_MAP_LIGHT_SIMULATION"
        | "ENABLE_LEGACY_DLIGHTS" | "ENABLE_VERTEX_DLIGHTS"
        | "ENABLE_CLUSTERED_LITE_DLIGHTS" | "ENABLE_AREA_LIGHTS"
        | "ENABLE_IRRADIANCE_VOLUME" | "ENABLE_VOXEL_GI"
        | "ENABLE_LOCAL_SHADOWS" | "ENABLE_CASCADED_SHADOWS" | "ENABLE_CSM_COVERAGE"
        | "ENABLE_PLANAR_REFLECTIONS" | "ENABLE_LEGACY_FOG"
        | "ENABLE_JUMP_SHADE"));
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA analytic far ocean (mesh free)"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader, entry_point: Some("vs_ocean_far"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None, ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples, ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader, entry_point: Some("fs_ocean_far"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants, ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format, blend: None, write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

impl Renderer {
    /// Enqueue work only when infinite far water is actively requested, with
    /// the same cancellation/worker machinery as other optional pipelines.
    pub(in crate::renderer) fn ensure_ocean_far_pipeline(&mut self) {
        if !self.ocean_sky_infinite || !self.ocean_enabled { return; }
        let Some(world) = self.world.as_ref() else { return; };
        if !world.ocean_sky_regions.iter().any(Option::is_some) { return; }
        let variant = world.active_pipeline_variant;
        if !variant.ocean || variant.family() != WorldShaderFamily::Enhanced {return;}
        if self.world.as_ref().and_then(|world|
            world.pipeline_variants.get(&variant))
            .is_some_and(|p| p.ocean_far_pipeline.is_some()) {return;}
        let format = self.scene_format();
        let samples = self.msaa_samples;
        let key = PipelineJobKey::new("ocean-far", 0, pipeline_hash(&(variant,format,samples)));
        if let Some(pipeline) = self.pipeline_jobs.take_ready::<wgpu::RenderPipeline>(key) {
            if let Some(pipelines) = self.world.as_mut()
                .and_then(|world| world.pipeline_variants.get_mut(&variant)) {
                pipelines.ocean_far_pipeline = Some(pipeline);
            }
            return;
        }
        let (layout, shader) = if variant.ray_traced_shadows {
            let Some(rt) = self.ray_traced_shadows.as_ref() else {return;};
            (rt.world_pipeline_layout.clone(), rt.world_shader_for_variant(variant).clone())
        } else {
            (self.world_pipeline_layout.clone(), self.world_shader.clone())
        };
        let device = self.device.clone();
        self.pipeline_jobs.request(key, "analytic far ocean", move || {
            create_ocean_far_pipeline(&device,&layout,&shader,format,samples,variant)
        });
    }
}

/// Submerged Hybrid: Blue Water's own clipmap vertices (`OceanVertex`) through
/// the Godot ocean material, `vs_ocean_native` with the ocean key's `fs_main`
/// state, so only the mesh differs from the Godot water drawn above. With
/// `wireframe`, its r_showtris overlay (`fs_wireframe`, lines, no depth test).
pub(in crate::renderer) fn create_ocean_native_mesh_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    variant: WorldShaderVariantKey,
    wireframe: bool,
) -> wgpu::RenderPipeline {
    // Exactly what `create_world_pipeline` gives `ocean_pipeline_key()`: the
    // base-pass constants, alpha test on (a Transparent class), no RT in the VS.
    let mut constants = variant.base_pass_key().compilation_constants();
    constants.push(("ENABLE_ALPHA_TEST", 1.0));
    let vertex_constants = constants.iter().copied()
        .filter(|(name, _)| !matches!(*name, "ENABLE_RAY_TRACED_SHADOWS"))
        .collect::<Vec<_>>();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if wireframe { "JKA submerged native ocean wireframe" } else { "JKA submerged native ocean mesh" }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader, entry_point: Some("vs_ocean_native"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &vertex_constants, ..Default::default()
            },
            buffers: &[Some(blue_water_gpu::geometry::OceanVertex::LAYOUT)],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: if wireframe { wgpu::PolygonMode::Line } else { wgpu::PolygonMode::Fill },
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(!wireframe),
            depth_compare: Some(if wireframe { wgpu::CompareFunction::Always } else { camera_depth_compare() }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples, ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader, entry_point: Some(if wireframe { "fs_wireframe" } else { "fs_main" }),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: if wireframe { &[] } else { constants.as_slice() }, ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format, blend: None, write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

impl Renderer {
    /// Hybrid r_hybridUnderwaterMode 4 draws the native mesh with the Godot
    /// material while submerged. Compile it as soon as that mode is live, so it
    /// is ready before the camera first dives; the overlay only on r_showtris.
    /// The submerged Hybrid native mesh can draw: its pipeline is compiled and
    /// the native surface, whose slots build its indices, exists.
    pub(in crate::renderer) fn native_submerged_mesh_ready(&self) -> bool {
        self.world.as_ref()
            .and_then(|world| world.pipeline_variants.get(&world.active_pipeline_variant))
            .is_some_and(|variant| variant.ocean_native_mesh_pipeline.is_some())
            && self.ocean.as_ref().is_some_and(|gpu| gpu.original_surface_prepared())
    }

    pub(in crate::renderer) fn ensure_ocean_native_mesh_pipelines(&mut self, wireframe: bool) {
        let Some(world) = self.world.as_ref() else { return; };
        let variant = world.active_pipeline_variant;
        if !variant.ocean || variant.family() != WorldShaderFamily::Enhanced { return; }
        let Some(compiled) = world.pipeline_variants.get(&variant) else { return; };
        let wanted = [compiled.ocean_native_mesh_pipeline.is_none(),
            wireframe && self.wireframe_supported && compiled.ocean_native_mesh_wireframe_pipeline.is_none()];
        let format = self.scene_format();
        let samples = self.msaa_samples;
        for (wire, needed) in [false, true].into_iter().zip(wanted) {
            if !needed { continue; }
            let key = PipelineJobKey::new("ocean-native-mesh", u32::from(wire),
                pipeline_hash(&(variant, format, samples, wire)));
            if let Some(pipeline) = self.pipeline_jobs.take_ready::<wgpu::RenderPipeline>(key) {
                if let Some(pipelines) = self.world.as_mut()
                    .and_then(|world| world.pipeline_variants.get_mut(&variant)) {
                    if wire { pipelines.ocean_native_mesh_wireframe_pipeline = Some(pipeline); }
                    else { pipelines.ocean_native_mesh_pipeline = Some(pipeline); }
                }
                continue;
            }
            let (layout, shader) = if variant.ray_traced_shadows {
                let Some(rt) = self.ray_traced_shadows.as_ref() else { return; };
                (rt.world_pipeline_layout.clone(), rt.world_shader_for_variant(variant).clone())
            } else {
                (self.world_pipeline_layout.clone(), self.world_shader.clone())
            };
            let device = self.device.clone();
            self.pipeline_jobs.request(key, "submerged native ocean mesh", move || {
                create_ocean_native_mesh_pipeline(&device, &layout, &shader, format, samples, variant, wire)
            });
        }
    }
}

pub(in crate::renderer) fn create_sky_box_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
) -> [wgpu::RenderPipeline; 6] {
    ["rt", "bk", "lf", "ft", "up", "dn"].map(|face| {
        let vertex = format!("vs_sky_box_{face}");
        let fragment = format!("fs_sky_box_{face}");
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("JKA sky box face pipeline"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(&vertex),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(camera_depth_compare()),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(&fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: crate::pipeline_cache::get().as_ref(),
        })
    })
}

/// One cloud-layer PSO (`vs_sky_cloud` / `fs_sky_cloud`) for an authored blend.
pub(in crate::renderer) fn create_sky_cloud_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
    blend: BlendMode,
) -> wgpu::RenderPipeline {
    let blend = match blend {
        BlendMode::Opaque => None,
        BlendMode::Custom(src, dst) => {
            // Same glBlendFunc(src, dst) mapping as create_world_pipeline.
            let component = wgpu::BlendComponent {
                src_factor: gpu_blend_factor(src),
                dst_factor: gpu_blend_factor(dst),
                operation: wgpu::BlendOperation::Add,
            };
            Some(wgpu::BlendState { color: component, alpha: component })
        }
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA sky cloud layer pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_sky_cloud"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_sky_cloud"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

/// Sky stage that the box path draws as a cloud layer (tcGen sky cloud).
pub(in crate::renderer) fn is_sky_cloud_stage(source: &DrawBatch) -> bool {
    source.pipeline.class == DrawClass::Sky && matches!(source.tc_gen, TcGen::SkyCloud(_))
}

impl Renderer {
    /// Builds the sky box face PSOs for the current scene format, sample count
    /// and world shader family (six tiny pipelines), plus one cloud-layer PSO per
    /// blend the loaded map's cloud stages use. Rebuilt only when one of those
    /// changes or a different map is loaded.
    pub(in crate::renderer) fn ensure_sky_box_pipelines(&mut self, enhanced: bool) {
        let key = (self.scene_format(), self.msaa_samples, enhanced);
        let (layout, shader) = if enhanced {
            (&self.world_pipeline_layout, &self.world_shader)
        } else {
            (&self.world_pipeline_layout_lean, &self.world_shader_lean)
        };
        if !self.sky_box_pipelines.as_ref().is_some_and(|(built, _)| *built == key) {
            let pipelines = create_sky_box_pipelines(&self.device, layout, shader, key.0, key.1);
            self.sky_box_pipelines = Some((key, pipelines));
            self.sky_cloud_pipelines.clear();
            self.sky_cloud_world = None;
        }
        let Some(world) = self.world.as_ref() else { return };
        if self.sky_cloud_world.as_ref() == Some(&world.index_buffer) {
            return;
        }
        for batch in world.coarse_batches.iter().filter(|batch| is_sky_cloud_stage(&batch.source)) {
            let blend = batch.source.pipeline.blend;
            self.sky_cloud_pipelines.entry(blend).or_insert_with(|| {
                create_sky_cloud_pipeline(&self.device, layout, shader, key.0, key.1, blend)
            });
        }
        self.sky_cloud_world = Some(world.index_buffer.clone());
    }
}

/// r_fragmentShaderMode 2: plain stages draw with `vs_world_minimal` /
/// `fs_world_minimal` (default world vertex input, fs_main semantics) when the
/// variant allows it. Fused additive, three-stage sRGB and autoSprite2 stages keep
/// their own entry points.
pub(in crate::renderer) fn material_minimal_entry(
    variant: WorldShaderVariantKey,
    key: &PipelineKey,
) -> bool {
    variant.minimal_entry_compatible()
        && material_specialization_eligible(key)
        && !key.legacy_srgb_three_stage
        && !key.auto_sprite2
}

/// Base-pass pipelines that use the shared `shade_surface` material code. Sky and
/// fused additive entry points keep the general program, and so does the key the
/// ocean PSO takes over while the ocean is on. Every other stage is specialized,
/// transparent ones included, as in TaystJK: with the classification key
/// (`MaterialFragmentKey::for_variant`) blended stages collapse into a few PSOs.
pub(in crate::renderer) fn material_specialization_eligible(key: &PipelineKey) -> bool {
    key.class != DrawClass::Sky && key.additive_fusion_count < 2 && *key != ocean_pipeline_key()
}

pub(in crate::renderer) fn material_specialization_jobs<'a>(
    batches: impl Iterator<Item = &'a WorldBatch>,
) -> Vec<(PipelineKey, MaterialFragmentKey)> {
    batches
        .filter(|batch| material_specialization_eligible(&batch.source.pipeline))
        .map(|batch| (batch.source.pipeline, batch.fragment_key))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Looked up for every world batch every frame (`base_pipeline`): use the fold
/// hasher, not SipHash, for the ~60-byte key.
pub(in crate::renderer) type SpecializedWorldPipelines = std::collections::HashMap<
    (PipelineKey, MaterialFragmentKey),
    (MaterialFragmentKey, wgpu::RenderPipeline),
    std::hash::BuildHasherDefault<crate::scene::FoldHasher>,
>;

/// Narrows each (pipeline, full material key) job for `variant` and returns the
/// distinct PSOs to build plus, per job, the index of its PSO.
fn specialized_pipeline_plan(
    jobs: &[(PipelineKey, MaterialFragmentKey)],
    variant: WorldShaderVariantKey,
) -> (Vec<(PipelineKey, MaterialFragmentKey)>, Vec<usize>) {
    let mut unique = Vec::new();
    let mut index = std::collections::HashMap::new();
    let slots = jobs
        .iter()
        .map(|&(key, material)| {
            let narrowed = super::programs::program_key(variant, key, material);
            *index.entry((key, narrowed)).or_insert_with(|| {
                unique.push((key, narrowed));
                unique.len() - 1
            })
        })
        .collect();
    (unique, slots)
}

fn specialized_pipeline_map(
    jobs: &[(PipelineKey, MaterialFragmentKey)],
    unique: &[(PipelineKey, MaterialFragmentKey)],
    slots: &[usize],
    pipelines: Vec<wgpu::RenderPipeline>,
) -> SpecializedWorldPipelines {
    jobs.iter()
        .zip(slots)
        .map(|(&job, &slot)| (job, (unique[slot].1, pipelines[slot].clone())))
        .collect()
}

/// r_fragmentShaderMode 1 on the synchronous paths (map load, format/MSAA rebuild).
/// Empty unless the variant asks for specialization.
pub(in crate::renderer) fn create_specialized_world_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    lean_shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    jobs: &[(PipelineKey, MaterialFragmentKey)],
    variant: WorldShaderVariantKey,
) -> SpecializedWorldPipelines {
    if !variant.material_specialization {
        return Default::default();
    }
    let started = Instant::now();
    let (unique, slots) = specialized_pipeline_plan(jobs, variant);
    let modules = super::programs::ProgramModules::default();
    let pipelines = compile_pipeline_jobs("world-specialized", unique.clone(), |&(key, material)| {
        modules.pipeline(device, layout, shader, lean_shader, surface_format, msaa_samples, variant, key, material)
    });
    log_material_specialization(jobs, &unique, variant, started);
    specialized_pipeline_map(jobs, &unique, &slots, pipelines)
}

fn log_material_specialization(
    jobs: &[(PipelineKey, MaterialFragmentKey)],
    pipelines: &[(PipelineKey, MaterialFragmentKey)],
    variant: WorldShaderVariantKey,
    started: Instant,
) {
    let count = |class| pipelines.iter().filter(|(key, _)| key.class == class).count();
    rverbose!(1, "Fragment specialization: {} material key(s) -> {} specialized PSO(s)         (opaque {}, mask {}, transparent {}) in {:.1} ms",
        jobs.len(), pipelines.len(), count(DrawClass::Opaque), count(DrawClass::Mask),
        count(DrawClass::Transparent), started.elapsed().as_secs_f64() * 1000.0);
    use super::programs::{StageProgram, WorldProgramMode, program_key, program_variant};
    if variant.program_mode != WorldProgramMode::Specialized {
        let mut families = BTreeMap::<StageProgram, usize>::new();
        let mut fallbacks = BTreeMap::<&str, usize>::new();
        for &(pipeline, raw) in jobs {
            let choice = program_key(variant, pipeline, raw);
            *families.entry(choice.program).or_default() += 1;
            if matches!(choice.program, StageProgram::Specialized | StageProgram::SpecializedLean) {
                *fallbacks.entry(super::programs::fallback_reason(variant, pipeline, raw)).or_default() += 1;
            }
        }
        let programs = pipelines.iter().map(|&(pipeline, material)| (
            program_variant(variant, material), material, pipeline.class != DrawClass::Opaque,
            pipeline.auto_sprite2, pipeline.legacy_srgb_three_stage,
        )).collect::<BTreeSet<_>>();
        println!("[WORLD STAGE RECIPES] mode={} prepared_recipes={} program_pairs={} pipelines={} coverage={families:?} fallbacks={fallbacks:?} compile_ms={:.1}",
            super::programs::active_mode(variant), jobs.len(), programs.len(), pipelines.len(), started.elapsed().as_secs_f64()*1000.0);
    }
}

/// A cold-path snapshot of the compiled pass identities currently retained by
/// the map. The scene-format/MSAA rebuild replaces the complete variant set;
/// include both in identity anyway to protect future cache refactors.
#[derive(Default)]
pub(in crate::renderer) struct WorldPipelineReuseCatalog {
    pipelines: std::collections::HashMap<WorldPassPipelineIdentity, wgpu::RenderPipeline>,
}

impl WorldPipelineReuseCatalog {
    pub fn from_variants(
        variants: &BTreeMap<WorldShaderVariantKey, WorldPipelineVariant>,
        format: wgpu::TextureFormat, samples: u32,
    ) -> Self {
        let mut catalog = Self::default();
        for (&variant, set) in variants {
            for (&key, pipeline) in &set.pipelines {
                let fog = variant.legacy_fog
                    && !(variant.ocean && key == ocean_pipeline_key());
                catalog.insert(WorldPassPipelineIdentity::new(
                    variant, WorldPipelinePass::Base { authored_fog: fog },
                    key, format, samples,
                ), pipeline);
            }
                    for (&(key, _), (material, pipeline)) in &set.specialized_pipelines {
                catalog.insert(WorldPassPipelineIdentity::specialized(
                    variant, variant.legacy_fog, key, *material,
                    super::programs::uses_minimal_entry(*material), format, samples,
                ), pipeline);
            }
            for (&original, pipeline) in &set.legacy_dlight_pipelines {
                let mut key = original;
                key.depth_write = false;
                key.depth_equal = true;
                catalog.insert(WorldPassPipelineIdentity::new(
                    variant, WorldPipelinePass::LegacyDlight, key, format, samples,
                ), pipeline);
            }
            for (&(original, writes_depth), pipeline) in &set.fog_pass_pipelines {
                catalog.insert(WorldPassPipelineIdentity::new(
                    variant, WorldPipelinePass::LegacyFog,
                    legacy_fog_pass_pipeline_key(original, writes_depth), format, samples,
                ), pipeline);
            }
            for (&original, pipeline) in &set.reflection_pipelines {
                catalog.insert(WorldPassPipelineIdentity::new(
                    variant, WorldPipelinePass::Reflection { authored_fog: variant.legacy_fog },
                    reflected_pipeline_key(original), format, 1,
                ), pipeline);
            }
            for (&(original, writes_depth), pipeline) in &set.reflection_fog_pass_pipelines {
                catalog.insert(WorldPassPipelineIdentity::new(
                    variant, WorldPipelinePass::ReflectionFog,
                    reflected_pipeline_key(legacy_fog_pass_pipeline_key(original, writes_depth)),
                    format, 1,
                ), pipeline);
            }
        }
        catalog
    }

    fn insert(&mut self, identity: WorldPassPipelineIdentity, pipeline: &wgpu::RenderPipeline) {
        self.pipelines.entry(identity).or_insert_with(|| pipeline.clone());
    }
}

fn reflected_pipeline_key(mut original: PipelineKey) -> PipelineKey {
    original.cull = match original.cull {
        CullMode::None => CullMode::None,
        CullMode::Front => CullMode::Back,
        CullMode::Back => CullMode::Front,
    };
    original
}

/// Runtime compilations always execute on the one dedicated world worker.
/// Checking cancellation between actual native PSOs avoids spending another
/// minute finishing an obsolete settings permutation. Native driver calls
/// cannot be interrupted, and a cancelled partial variant is never activated.
fn compile_cached_world_pass<K: Sync + std::fmt::Debug>(
    label: &str,
    jobs: &[K],
    catalog: &WorldPipelineReuseCatalog,
    cancelled: &std::sync::atomic::AtomicBool,
    identity: impl Fn(&K) -> WorldPassPipelineIdentity,
    create: impl Fn(&K) -> wgpu::RenderPipeline,
) -> Option<Vec<wgpu::RenderPipeline>> {
    let start = Instant::now();
    let mut reused = 0usize;
    let mut created = 0usize;
    let mut outputs = Vec::with_capacity(jobs.len());
    let mut created_cache = std::collections::HashMap::new();
    for (i, job) in jobs.iter().enumerate() {
        if cancelled.load(Ordering::Acquire) {
            rverbose!(1, "Renderer pipeline compile [{label}]: cancelled after {created} new / {reused} reused PSO(s)");
            return None;
        }
        let key = identity(job);
        if let Some(existing) = catalog.pipelines.get(&key)
            .or_else(|| created_cache.get(&key))
        {
            reused += 1;
            outputs.push(existing.clone());
            continue;
        }
        let detail = format!("{job:?}");
        let concise = detail.split("class: ").nth(1)
            .and_then(|rest| rest.split(", cull:").next())
            .unwrap_or("world pipeline");
        let _stage = crate::pipeline_jobs::WorldPipelineStageGuard::begin(
            (i + 1) as u32, jobs.len() as u32, format!("{label}: {concise}"),
        );
        let began = Instant::now();
        // This worker's own job is one of the active compiles.
        let others_at_start = crate::pipeline_jobs::active_compiles().saturating_sub(1);
        let pipeline = create(job);
        let elapsed = began.elapsed();
        if elapsed.as_secs_f32() >= 1.0 {
            eprintln!("[SLOW WORLD PIPELINE] {label} {}/{} {} {:.1} ms (other compiles: {} at start, {} at end)",
                i + 1, jobs.len(), concise, elapsed.as_secs_f64() * 1000.0, others_at_start,
                crate::pipeline_jobs::active_compiles().saturating_sub(1));
        }
        created_cache.insert(key, pipeline.clone());
        outputs.push(pipeline);
        created += 1;
    }
    rverbose!(1, "Renderer pipeline compile [{label}]: {} new / {} reused in {:.1} ms wall",
        created, reused, start.elapsed().as_secs_f64() * 1000.0);
    Some(outputs)
}

pub(in crate::renderer) fn create_world_pipeline_variant_from_plan(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    lean_shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shader_variant: WorldShaderVariantKey,
    plan: &WorldPipelineCompilePlan,
    reusable: &WorldPipelineReuseCatalog,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Option<WorldPipelineVariant> {
    let mut base_jobs = plan.base_keys.iter().copied()
        .map(|key| (key, shader_variant.legacy_fog)).collect::<Vec<_>>();
    if shader_variant.ocean {
        base_jobs.push((ocean_pipeline_key(), false));
    }
    // The variant's enabled features, so a slow-compile line says what it built.
    let base_label = format!("world-runtime [{}]", shader_variant.short_label());
    let pipelines = compile_cached_world_pass(&base_label, &base_jobs, reusable, cancelled,
        |&(key, fog)| WorldPassPipelineIdentity::new(
            shader_variant, WorldPipelinePass::Base { authored_fog: fog },
            key, surface_format, msaa_samples),
        |&(key, fog)| create_world_pipeline(
            device, layout, shader, surface_format, key, msaa_samples,
            fog, false, Some(shader_variant.base_pass_key()), false, None))?
        .into_iter().zip(base_jobs).map(|(pipeline,(key,_))| (key,pipeline)).collect();

    let legacy_dlight_pipelines = if shader_variant.legacy_dlights {
        let originals = &plan.legacy_dlight_keys;
        compile_cached_world_pass("legacy-dlight-runtime", originals, reusable, cancelled,
            |&original| {
                let mut key = original;
                key.depth_write = false;
                key.depth_equal = true;
                WorldPassPipelineIdentity::new(shader_variant,
                    WorldPipelinePass::LegacyDlight, key, surface_format, msaa_samples)
            },
            |&original| {
                let mut key = original;
                key.depth_write = false;
                key.depth_equal = true;
                create_world_pipeline(device, layout, shader, surface_format,
                    key, msaa_samples, false, false, Some(shader_variant), true, None)
            })?.into_iter().zip(originals.iter().copied())
            .map(|(pipeline,key)| (key,pipeline)).collect()
    } else { BTreeMap::new() };

    let fog_pass_pipelines = if shader_variant.legacy_fog {
        let jobs = &plan.fog_jobs;
        compile_cached_world_pass("legacy-fog-runtime", jobs, reusable, cancelled,
            |&(original, writes_depth)| WorldPassPipelineIdentity::new(
                shader_variant, WorldPipelinePass::LegacyFog,
                legacy_fog_pass_pipeline_key(original, writes_depth), surface_format, msaa_samples),
            |&(original, writes_depth)| create_world_pipeline(
                device, layout, shader, surface_format,
                legacy_fog_pass_pipeline_key(original, writes_depth),
                msaa_samples, true, true, Some(shader_variant), false, None))?
            .into_iter().zip(jobs.iter().copied())
            .map(|(pipeline,key)| (key,pipeline)).collect()
    } else { BTreeMap::new() };

    let reflection_pipelines = if plan.has_planar_reflectors && shader_variant.planar_reflections {
        let originals = &plan.base_keys;
        compile_cached_world_pass("reflection-runtime", originals, reusable, cancelled,
            |&original| WorldPassPipelineIdentity::new(
                shader_variant, WorldPipelinePass::Reflection { authored_fog: shader_variant.legacy_fog },
                reflected_pipeline_key(original), surface_format, 1),
            |&original| create_world_pipeline(
                device, layout, shader, surface_format, reflected_pipeline_key(original),
                1, shader_variant.legacy_fog, false, Some(shader_variant), false, None))?
            .into_iter().zip(originals.iter().copied())
            .map(|(pipeline,key)| (key,pipeline)).collect()
    } else { BTreeMap::new() };

    let reflection_fog_pass_pipelines = if shader_variant.legacy_fog
        && plan.has_planar_reflectors && shader_variant.planar_reflections {
        let jobs = &plan.fog_jobs;
        compile_cached_world_pass("reflection-fog-runtime", jobs, reusable, cancelled,
            |&(original, writes_depth)| WorldPassPipelineIdentity::new(
                shader_variant, WorldPipelinePass::ReflectionFog,
                reflected_pipeline_key(legacy_fog_pass_pipeline_key(original, writes_depth)),
                surface_format, 1),
            |&(original, writes_depth)| create_world_pipeline(
                device, layout, shader, surface_format,
                reflected_pipeline_key(legacy_fog_pass_pipeline_key(original, writes_depth)),
                1, true, true, Some(shader_variant), false, None))?
            .into_iter().zip(jobs.iter().copied())
            .map(|(pipeline,key)| (key,pipeline)).collect()
    } else { BTreeMap::new() };

    let specialized_pipelines = if shader_variant.material_specialization {
        let started = Instant::now();
        let (unique, slots) = specialized_pipeline_plan(&plan.material_jobs, shader_variant);
        let modules = super::programs::ProgramModules::default();
        let label = format!("world-specialized-runtime [{}]", shader_variant.short_label());
        let compiled = compile_cached_world_pass(&label, &unique, reusable, cancelled,
            |&(key, material)| WorldPassPipelineIdentity::specialized(
                shader_variant, shader_variant.legacy_fog, key, material,
                super::programs::uses_minimal_entry(material), surface_format, msaa_samples),
            |&(key, material)| modules.pipeline(
                device, layout, shader, lean_shader, surface_format, msaa_samples, shader_variant, key, material))?;
        log_material_specialization(&plan.material_jobs, &unique, shader_variant, started);
        specialized_pipeline_map(&plan.material_jobs, &unique, &slots, compiled)
    } else {
        Default::default()
    };

    Some(WorldPipelineVariant {
        ocean_far_pipeline: None,
        ocean_native_mesh_pipeline: None,
        ocean_native_mesh_wireframe_pipeline: None,
        pipelines,
        legacy_dlight_pipelines,
        fog_pass_pipelines,
        reflection_pipelines,
        reflection_fog_pass_pipelines,
        specialized_pipelines,
        slot_pipelines: Vec::new(),
    })
}

pub(in crate::renderer) fn create_shader_complexity_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    samples: u32,
    keys: &[PipelineKey],
    variant: Option<WorldShaderVariantKey>,
) -> BTreeMap<PipelineKey, wgpu::RenderPipeline> {
    // This shader module and all diagnostic PSOs exist ONLY in the optional
    // worker job. Normal BSP modules never see these declarations.
    let complexity_fragment = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("BSP shader complexity (diagnostics only)"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../../bsp_complexity.wgsl").into()),
    });
    let jobs = keys.to_vec();
    let pipelines = compile_pipeline_jobs("bsp-shader-complexity", jobs.clone(), |&key| {
        create_world_pipeline(device, layout, shader, surface_format, key, samples,
            false, false, variant.map(WorldShaderVariantKey::base_pass_key), false, Some(&complexity_fragment))
    });
    jobs.into_iter().zip(pipelines).collect()
}

pub(in crate::renderer) fn create_world_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    batches: &[WorldBatch],
    legacy_fog: bool,
    shader_variant: Option<WorldShaderVariantKey>,
) -> BTreeMap<PipelineKey, wgpu::RenderPipeline> {
    // OpenJK projects Legacy dlights in a separate additive pass. Keep the base
    // material variant free of the dlight loop even when the user selected
    // Legacy; the dedicated pass below compiles the original key.
    let base_shader_variant = shader_variant.map(WorldShaderVariantKey::base_pass_key);
    let mut jobs = batches
        .iter()
        .map(|batch| batch.source.pipeline)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|key| (key, legacy_fog))
        .collect::<Vec<_>>();
    if shader_variant.is_some_and(|key| key.ocean) {
        // Ocean never uses the legacy-fog entry point; appended last so it
        // replaces any batch pipeline that shares its key, as before.
        jobs.push((ocean_pipeline_key(), false));
    }
    let keys = jobs.iter().map(|&(key, _)| key).collect::<Vec<_>>();
    let compiled = compile_pipeline_jobs("world", jobs, |&(key, fog)| {
        create_world_pipeline(
            device,
            layout,
            shader,
            surface_format,
            key,
            msaa_samples,
            fog,
            false,
            base_shader_variant,
            false,
            None,
        )
    });
    keys.into_iter().zip(compiled).collect()
}

/// Worker count for parallel pipeline compilation. `JKA_PIPELINE_COMPILE_THREADS`
/// overrides it (1 = serial) for A/B timing; otherwise use most logical cores while
/// leaving one for the game thread.
pub(in crate::renderer) fn pipeline_compile_workers(jobs: usize) -> usize {
    static CONFIGURED: OnceLock<usize> = OnceLock::new();
    let configured = *CONFIGURED.get_or_init(|| {
        std::env::var("JKA_PIPELINE_COMPILE_THREADS")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|&count| count > 0)
            .unwrap_or_else(|| {
                thread::available_parallelism()
                    .map_or(4, |count| count.get())
                    .saturating_sub(1)
                    .clamp(1, 12)
            })
    });
    configured.min(jobs).max(1)
}

/// Runs `create` for every job, in parallel across scoped worker threads, and returns
/// the results in job order. wgpu pipeline creation is thread-safe, so the driver
/// compiles overlap instead of stalling the render thread one after another.
/// Logs the pipeline count and wall/summed/slowest compile times.
pub(in crate::renderer) fn compile_pipeline_jobs<K, T>(
    label: &str,
    jobs: Vec<K>,
    create: impl Fn(&K) -> T + Sync,
) -> Vec<T>
where
    K: Sync + std::fmt::Debug,
    T: Send,
{
    if jobs.is_empty() {
        return Vec::new();
    }
    let started = Instant::now();
    // Runtime-lazy world variants already execute inside the bounded persistent
    // pipeline pool. Do not fan one such job back out to 8-12 temporary threads
    // and steal every CPU core from a live 1000+ FPS game. Loading/rebuild-time
    // callers retain the wide parallel compiler.
    let current_thread = thread::current();
    let on_runtime_pipeline_worker = current_thread
        .name()
        .is_some_and(|name| name.starts_with("pipeline-compile-"));
    let workers = if on_runtime_pipeline_worker {
        1
    } else {
        pipeline_compile_workers(jobs.len())
    };
    let next = std::sync::atomic::AtomicUsize::new(0);
    let run_worker = || {
        let mut done = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(job) = jobs.get(index) else {
                break;
            };
            let _active = (!on_runtime_pipeline_worker)
                .then(crate::pipeline_jobs::ActiveCompileGuard::begin);
            let job_started = Instant::now();
            // The single world worker compiles a whole set of native PSOs as
            // one outer job. Publish the inner index and material class while
            // the driver is inside create_render_pipeline. This is diagnostics
            // only: it never changes the shader, ordering or pipeline key.
            let detail = format!("{job:?}");
            let concise = detail.split("class: ").nth(1)
                .and_then(|rest| rest.split(", cull:").next())
                .unwrap_or("world pipeline");
            let _stage_guard = (current_thread.name() == Some("pipeline-compile-world"))
                .then(|| crate::pipeline_jobs::WorldPipelineStageGuard::begin(
                    (index + 1) as u32, jobs.len() as u32,
                    format!("{label}: {concise}"),
                ));
            let value = create(job);
            let elapsed = job_started.elapsed();
            if on_runtime_pipeline_worker && elapsed.as_secs_f32() >= 1.0 {
                eprintln!("[SLOW WORLD PIPELINE] {label} {}/{} {} {:.1} ms",
                    index + 1, jobs.len(), concise, elapsed.as_secs_f64() * 1000.0);
            }
            done.push((index, value, elapsed));
        }
        done
    };
    let mut finished = if workers <= 1 {
        run_worker()
    } else {
        thread::scope(|scope| {
            let handles = (0..workers)
                .map(|_| std::thread::Builder::new().name("pipeline-world-scoped".into()).spawn_scoped(scope, &run_worker).expect("spawn named worker"))
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().expect("pipeline compile worker panicked"))
                .collect::<Vec<_>>()
        })
    };
    finished.sort_unstable_by_key(|&(index, ..)| index);
    let summed = finished
        .iter()
        .map(|(_, _, elapsed)| elapsed.as_secs_f64() * 1000.0)
        .sum::<f64>();
    let slowest = finished
        .iter()
        .map(|(_, _, elapsed)| elapsed.as_secs_f64() * 1000.0)
        .fold(0.0_f64, f64::max);
    rverbose!(
        1,
        "Renderer pipeline compile [{}]: {} pipeline(s) on {} worker(s) in {:.1} ms wall ({:.1} ms summed, {:.1} ms slowest, {:.1} ms avg)",
        label,
        finished.len(),
        workers,
        started.elapsed().as_secs_f64() * 1000.0,
        summed,
        slowest,
        summed / finished.len() as f64,
    );
    finished.into_iter().map(|(_, value, _)| value).collect()
}

pub(in crate::renderer) fn legacy_dlight_receives(source: &DrawBatch) -> bool {
    !source.texture_is_lightmap
        && source.pipeline.class != DrawClass::Sky
        && source.pipeline.blend == BlendMode::Opaque
}

pub(in crate::renderer) fn create_legacy_dlight_pass_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    batches: &[WorldBatch],
    shader_variant: WorldShaderVariantKey,
) -> BTreeMap<PipelineKey, wgpu::RenderPipeline> {
    let originals = batches
        .iter()
        .filter(|batch| legacy_dlight_receives(&batch.source))
        .map(|batch| batch.source.pipeline)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let compiled = compile_pipeline_jobs("legacy-dlight", originals.clone(), |&original| {
        let mut dlight_key = original;
        dlight_key.depth_write = false;
        dlight_key.depth_equal = true;
        create_world_pipeline(
            device,
            layout,
            shader,
            surface_format,
            dlight_key,
            msaa_samples,
            false,
            false,
            Some(shader_variant),
            true,
            None,
        )
    });
    originals.into_iter().zip(compiled).collect()
}

/// Distinct `(pipeline, surface_writes_depth)` pairs of the legacy fog passes, in the
/// order the batches first introduce them. Sky is skipped, as the passes never fog it.
pub(in crate::renderer) fn legacy_fog_pass_jobs(
    batches: &[WorldBatch],
) -> Vec<(PipelineKey, bool)> {
    let mut seen = BTreeSet::new();
    let mut jobs = Vec::new();
    let mut start = 0usize;
    while start < batches.len() {
        let mut end = start + 1;
        while end < batches.len() && same_world_surface(&batches[start], &batches[end]) {
            end += 1;
        }
        let surface_writes_depth = batches[start..end]
            .iter()
            .any(|batch| batch.source.pipeline.depth_write);
        for batch in &batches[start..end] {
            let original = batch.source.pipeline;
            if original.class != DrawClass::Sky && seen.insert((original, surface_writes_depth)) {
                jobs.push((original, surface_writes_depth));
            }
        }
        start = end;
    }
    jobs
}

pub(in crate::renderer) fn legacy_fog_pass_pipeline_key(
    source: PipelineKey,
    depth_equal: bool,
) -> PipelineKey {
    PipelineKey {
        legacy_srgb_three_stage: false,
        additive_fusion_count: 0,
        auto_sprite2: source.auto_sprite2,
        sky_outer: source.sky_outer,
        class: source.class,
        blend: BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha),
        cull: source.cull,
        offset: source.offset,
        depth_write: false,
        // OpenJK chooses FP_EQUAL for shaders whose material wrote depth and
        // ordinary depth testing for genuinely translucent/no-depth shaders.
        depth_equal,
    }
}

pub(in crate::renderer) fn same_world_surface(a: &WorldBatch, b: &WorldBatch) -> bool {
    a.source.vertices == b.source.vertices && a.source.bsp_shader_index == b.source.bsp_shader_index
}

pub(in crate::renderer) fn create_legacy_fog_pass_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    batches: &[WorldBatch],
    shader_variant: Option<WorldShaderVariantKey>,
) -> BTreeMap<(PipelineKey, bool), wgpu::RenderPipeline> {
    let jobs = legacy_fog_pass_jobs(batches);
    let compiled = compile_pipeline_jobs(
        "legacy-fog-pass",
        jobs.clone(),
        |&(original, writes_depth)| {
            create_world_pipeline(
                device,
                layout,
                shader,
                surface_format,
                legacy_fog_pass_pipeline_key(original, writes_depth),
                msaa_samples,
                true,
                true,
                shader_variant,
                false,
                None,
        )
        },
    );
    jobs.into_iter().zip(compiled).collect()
}

pub(in crate::renderer) fn ocean_pipeline_key() -> PipelineKey {
    PipelineKey {
        legacy_srgb_three_stage: false,
        additive_fusion_count: 0,
        auto_sprite2: false,
        sky_outer: false,
        class: DrawClass::Transparent,
        blend: BlendMode::Opaque,
        cull: CullMode::None,
        offset: false,
        depth_write: true,
        depth_equal: false,
    }
}

pub(in crate::renderer) fn create_reflection_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    batches: &[WorldBatch],
    legacy_fog: bool,
    shader_variant: WorldShaderVariantKey,
) -> BTreeMap<PipelineKey, wgpu::RenderPipeline> {
    let originals = batches
        .iter()
        .map(|batch| batch.source.pipeline)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let compiled = compile_pipeline_jobs("reflection", originals.clone(), |&original| {
        let mut reflected = original;
        reflected.cull = match reflected.cull {
            CullMode::None => CullMode::None,
            CullMode::Front => CullMode::Back,
            CullMode::Back => CullMode::Front,
        };
        create_world_pipeline(
            device,
            layout,
            shader,
            surface_format,
            reflected,
            1,
            legacy_fog,
            false,
            Some(shader_variant),
            false,
            None,
        )
    });
    originals.into_iter().zip(compiled).collect()
}

pub(in crate::renderer) fn create_reflection_fog_pass_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    batches: &[WorldBatch],
    shader_variant: WorldShaderVariantKey,
) -> BTreeMap<(PipelineKey, bool), wgpu::RenderPipeline> {
    let jobs = legacy_fog_pass_jobs(batches);
    let compiled = compile_pipeline_jobs(
        "reflection-fog-pass",
        jobs.clone(),
        |&(original, writes_depth)| {
            let mut reflected = legacy_fog_pass_pipeline_key(original, writes_depth);
            reflected.cull = match reflected.cull {
                CullMode::None => CullMode::None,
                CullMode::Front => CullMode::Back,
                CullMode::Back => CullMode::Front,
            };
            create_world_pipeline(
                device,
                layout,
                shader,
                surface_format,
                reflected,
                1,
                true,
                true,
                Some(shader_variant),
                false,
                None,
        )
        },
    );
    jobs.into_iter().zip(compiled).collect()
}

pub(in crate::renderer) fn create_depth_prepass_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    thermal: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA AO depth prepass pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if thermal { "fs_thermal" } else { "fs_main" }),
            compilation_options: Default::default(),
            targets: &[
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R32Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rg16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
            ],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}


/// Rain-only linear-depth pipeline: one R32F target instead of the general
/// prepass's linear-depth + motion-vector + reflection-policy MRT set.
pub(in crate::renderer) fn create_rain_depth_prepass_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    alpha_test: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if alpha_test {
            "JKA rain lean alpha-tested depth prepass pipeline"
        } else {
            "JKA rain lean depth prepass pipeline"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(if alpha_test { "vs_mask" } else { "vs_main" }),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if alpha_test { "fs_rain_mask" } else { "fs_rain" }),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R32Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

/// Depth-only world pipeline for the Hi-Z early pass. Opaque surfaces have no
/// fragment stage at all; alpha-tested ones run only the cutout test.
pub(in crate::renderer) fn create_hiz_prepass_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    alpha_test: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if alpha_test {
            "JKA Hi-Z early alpha-tested depth pipeline"
        } else {
            "JKA Hi-Z early depth pipeline"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(if alpha_test { "vs_mask" } else { "vs_main" }),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: alpha_test.then(|| wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_depth_mask"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

pub(in crate::renderer) fn create_depth_prepass_mask_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    thermal: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA alpha-tested depth prepass pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_mask"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if thermal { "fs_thermal_mask" } else { "fs_mask" }),
            compilation_options: Default::default(),
            targets: &[
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R32Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rg16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
            ],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

pub(in crate::renderer) fn create_entity_prepass_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vertex_layout: wgpu::VertexBufferLayout<'_>,
    alpha_test: bool,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_prepass"),
            compilation_options: Default::default(),
            buffers: &[Some(vertex_layout)],
        },
        // Same winding/culling as the entity colour pipelines.
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if alpha_test {
                "fs_prepass_mask"
            } else {
                "fs_prepass"
            }),
            compilation_options: Default::default(),
            targets: &[
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R32Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                // Entities have no per-bone history; keep TAA's existing
                // motion for these pixels rather than inventing camera-only motion.
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rg16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                }),
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
            ],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

pub(in crate::renderer) fn create_entity_rain_prepass_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vertex_layout: wgpu::VertexBufferLayout<'_>,
    alpha_test: bool,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_prepass"),
            compilation_options: Default::default(),
            buffers: &[Some(vertex_layout)],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if alpha_test {
                "fs_rain_prepass_mask"
            } else {
                "fs_rain_prepass"
            }),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R32Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

/// Depth-only entity pipeline for the Entity map shadow pass. Reuses each entity
/// shader's `vs_prepass` (same skinning as the colour pass) with the light's
/// view-projection in the camera slot. Opaque casters have no fragment stage;
/// alpha-tested ones discard through `fs_shadow_mask`. Two-sided with the same
/// slope-scaled bias as the BSP cascade casters, so thin open Ghoul2 surfaces
/// (hair, cloth, robes) still cast.
pub(in crate::renderer) fn create_entity_shadow_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vertex_layout: wgpu::VertexBufferLayout<'_>,
    alpha_test: bool,
    reverse_z: bool,
    unclipped_depth: bool,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_prepass"),
            compilation_options: Default::default(),
            buffers: &[Some(vertex_layout)],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: SHADOW_CASTER_CULL,
            unclipped_depth,
            ..Default::default()
        },
        // Same depth convention and bias as `create_shadow_pipeline`, so entities
        // and BSP casters agree inside one cascade layer.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(if reverse_z {
                wgpu::CompareFunction::GreaterEqual
            } else {
                wgpu::CompareFunction::LessEqual
            }),
            stencil: wgpu::StencilState::default(),
            bias: if reverse_z {
                wgpu::DepthBiasState::default()
            } else {
                wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                }
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: alpha_test.then(|| wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_shadow_mask"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}
