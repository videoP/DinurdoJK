//! World pipelines.
use crate::renderer::{
    camera_depth_compare, create_cull_debug_pipeline, create_ssr_visibility_resources,
    create_surface_inspector_pipeline, create_taa_post_pipeline, create_targets,
    create_world_pipeline, pipeline_hash, scene, thread, AlphaGen, BTreeMap, BTreeSet, BlendFactor,
    BlendMode, CullDebugMode, CullMode, DrawBatch, DrawClass, GpuVertex, Instant, MaterialUniform,
    OnceLock, Ordering, PipelineJobKey, PipelineKey, Renderer, RgbGen, TcGen, TcMod, Vec3,
    WorldBatch, WorldPipelineCompilePlan, WorldPipelineVariant, WorldShaderFamily,
    WorldShaderVariantKey, DEPTH_FORMAT, SHADOW_CASTER_CULL, VERTEX_ATTRIBUTES,
};

impl Renderer {
    /// Format the gameplay scene renders into, ignoring the asset viewer.
    pub(in crate::renderer) fn gameplay_scene_format(&self) -> wgpu::TextureFormat {
        if self.hdr_enabled || self.bloom_enabled || self.halation_enabled {
            wgpu::TextureFormat::Rgba16Float
        } else {
            self.config.format
        }
    }

    /// The asset viewer draws its model straight into the swapchain image with
    /// no post pass, so its pipelines and MSAA target must use the surface
    /// format even while HDR / bloom / halation are configured.
    pub(in crate::renderer) fn scene_format(&self) -> wgpu::TextureFormat {
        if self.asset_preview_mode {
            self.config.format
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
        // Render-target compatibility changes (MSAA / scene format) invalidate
        // every cached render pipeline. Rebuild only the active specialized
        // variant plus the known-fast baseline; future settings combinations
        // are created lazily again on demand.
        //
        // Keep this selection identical to activate_world_pipeline_variant():
        // an RT key must always be paired with the lazily-created RT WGSL and
        // RT group-3 layout. Passing the RT specialization constant to the stock
        // BSP shader is invalid because that shader intentionally does not
        // declare ENABLE_RAY_TRACED_SHADOWS.
        let mut key = self.world_shader_variant_key();
        if key.ray_traced_shadows && !self.ensure_ray_traced_shadow_resources() {
            key.ray_traced_shadows = false;
            key.ray_traced_sun = false;
        }
        let scene_format = self.scene_format();
        let samples = self.msaa_samples;
        let (layout, shader) = if key.ray_traced_shadows {
            let Some(rt) = self.ray_traced_shadows.as_ref() else {
                return;
            };
            match key.family() {
                WorldShaderFamily::Enhanced => (&rt.world_pipeline_layout, &rt.world_shader),
                WorldShaderFamily::Lean => (&rt.world_pipeline_layout_lean, &rt.world_shader_lean),
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
            (
                pipelines,
                legacy_dlight_pipelines,
                fog_pass_pipelines,
                reflection_pipelines,
                reflection_fog_pass_pipelines,
            )
        };

        self.world_variant_pending = None;
        if let Some(world) = &mut self.world {
            world.pipeline_variants.clear();
            world.pipeline_variants.insert(
                key,
                WorldPipelineVariant {
                    pipelines,
                    legacy_dlight_pipelines,
                    fog_pass_pipelines,
                    reflection_pipelines,
                    reflection_fog_pass_pipelines,
                },
            );
            world.active_pipeline_variant = key;
            // MSAA/scene-format changed: drop the fast set, it is recompiled on
            // demand the next time the frame plan selects FastBaseline.
            world.fast_pipelines.clear();
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
    /// Recreating the targets also recompiles the whole world pipeline set, which
    /// costs hundreds of milliseconds. Several independent settings request it —
    /// HDR, bloom, SSAO, SSR, depth of field, cloud resolution, MSAA — so a burst
    /// of changes arriving together (a quality preset sends about forty cvars at
    /// once) would otherwise pay that cost once per setting. Collapsing them to a
    /// single rebuild before the next frame turns a multi-second stall into one.
    pub(in crate::renderer) fn request_scene_rebuild(&mut self) {
        self.scene_rebuild_pending = true;
    }

    pub(in crate::renderer) fn apply_pending_scene_rebuild(&mut self) {
        if std::mem::take(&mut self.scene_rebuild_pending) {
            self.rebuild_scene_targets_and_pipelines();
        }
    }

    pub(in crate::renderer) fn rebuild_scene_targets_and_pipelines(&mut self) {
        self.invalidate_rt_sun_shadow_depth();
        // Companion scene textures/pipelines share the gameplay scene format and
        // MSAA count. Recreate them lazily after this rebuild; the presentation
        // worker keeps showing its last safe texture until the new ring arrives.
        self.companion_scene_targets.clear();
        let scene_format = self.scene_format();
        let samples = self.msaa_samples;
        self.targets = create_targets(
            &self.device,
            self.config.width,
            self.config.height,
            scene_format,
            samples,
            self.bloom_enabled || self.halation_enabled,
            self.depth_of_field_strength > 0.001,
            self.ssao_enabled,
            self.ssr_enabled,
            self.cloud_render_resolution,
        );
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
        self.weather
            .rain
            .rebuild_render_pipelines(&self.device, scene_format, samples);
        self.grass_renderer
            .rebuild_pipeline(&self.device, scene_format, samples);
        self.surface_sprite_effect_renderer
            .rebuild_pipelines(&self.device, scene_format, samples);
        self.dynamic_model_renderer
            .rebuild_pipelines(&self.device, scene_format, samples);
        self.dynamic_model_renderer
            .prewarm_core_model_pipelines(&mut self.pipeline_jobs, &self.device);
        // One shared spray pipeline serves every ocean; recompiled on demand by
        // ensure_lazy_scene_pipelines only while some ocean has sea spray on.
        self.ocean_spray_pipeline = None;
        self.rebuild_planar_reflection_resources();
        // TAA resolve and diagnostic pipelines depend on the scene target
        // format/MSAA; drop them and let ensure_lazy_scene_pipelines recompile
        // only the ones the current settings actually use.
        self.taa_post_pipeline = None;
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
        self.rebuild_world_pipelines();
        self.cull_debug_pipeline = None;
        // All wireframe pipelines are diagnostic-only and are rebuilt lazily on
        // demand after a scene-format/MSAA change.
        self.wireframe_pipeline = None;
        self.world_wireframe_pipelines.clear();
        self.fast_world_wireframe_pipeline = None;
        self.debug_volumes.clear_pipelines();
        self.surface_inspector_pipeline = None;
    }

    /// Compiles the pipelines that are only needed while TAA, the surface
    /// inspector, or the cull-rejection overlay are active. Runs on the cold
    /// pre-frame path so the frame recorder can borrow them immutably.
    pub(in crate::renderer) fn ensure_lazy_scene_pipelines(&mut self) {
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

        if self.cull_debug_mode == CullDebugMode::RejectionReasons
            && self.cull_debug_pipeline.is_none()
        {
            let key = PipelineJobKey::new("cull-debug", 0, pipeline_hash(&(scene_format, samples)));
            if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
                self.cull_debug_pipeline = Some(pipeline);
            } else {
                let device = self.device.clone();
                let layout = self.cull_debug_pipeline_layout.clone();
                let shader = self.cull_debug_shader.clone();
                self.pipeline_jobs
                    .request(key, "cull rejection debug", move || {
                        create_cull_debug_pipeline(&device, &layout, &shader, scene_format, samples)
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
    out.color = source.color;
    out.params[0] = source.alpha_cutoff;
    out.params[1] = source.parallax_depth;
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

pub(in crate::renderer) fn create_world_pipeline_variant_from_plan(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shader_variant: WorldShaderVariantKey,
    plan: &WorldPipelineCompilePlan,
) -> WorldPipelineVariant {
    let mut base_shader_variant = shader_variant;
    base_shader_variant.legacy_dlights = false;
    let mut base_jobs = plan
        .base_keys
        .iter()
        .copied()
        .map(|key| (key, shader_variant.legacy_fog))
        .collect::<Vec<_>>();
    if shader_variant.ocean {
        base_jobs.push((ocean_pipeline_key(), false));
    }
    let base_keys = base_jobs.iter().map(|&(key, _)| key).collect::<Vec<_>>();
    let pipelines = compile_pipeline_jobs("world-runtime", base_jobs, |&(key, fog)| {
        create_world_pipeline(
            device,
            layout,
            shader,
            surface_format,
            key,
            msaa_samples,
            fog,
            false,
            Some(base_shader_variant),
            false,
        )
    });
    let pipelines = base_keys.into_iter().zip(pipelines).collect();

    let legacy_dlight_pipelines = if shader_variant.legacy_dlights {
        let originals = plan.legacy_dlight_keys.clone();
        let compiled =
            compile_pipeline_jobs("legacy-dlight-runtime", originals.clone(), |&original| {
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
                )
            });
        originals.into_iter().zip(compiled).collect()
    } else {
        BTreeMap::new()
    };

    let fog_pass_pipelines = if shader_variant.legacy_fog {
        let jobs = plan.fog_jobs.clone();
        let compiled = compile_pipeline_jobs(
            "legacy-fog-runtime",
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
                    Some(shader_variant),
                    false,
                )
            },
        );
        jobs.into_iter().zip(compiled).collect()
    } else {
        BTreeMap::new()
    };

    let reflection_pipelines = if plan.has_planar_reflectors && shader_variant.planar_reflections {
        let originals = plan.base_keys.clone();
        let compiled =
            compile_pipeline_jobs("reflection-runtime", originals.clone(), |&original| {
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
                    shader_variant.legacy_fog,
                    false,
                    Some(shader_variant),
                    false,
                )
            });
        originals.into_iter().zip(compiled).collect()
    } else {
        BTreeMap::new()
    };

    let reflection_fog_pass_pipelines = if shader_variant.legacy_fog
        && plan.has_planar_reflectors
        && shader_variant.planar_reflections
    {
        let jobs = plan.fog_jobs.clone();
        let compiled = compile_pipeline_jobs(
            "reflection-fog-runtime",
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
                )
            },
        );
        jobs.into_iter().zip(compiled).collect()
    } else {
        BTreeMap::new()
    };

    WorldPipelineVariant {
        pipelines,
        legacy_dlight_pipelines,
        fog_pass_pipelines,
        reflection_pipelines,
        reflection_fog_pass_pipelines,
    }
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
    let base_shader_variant = shader_variant.map(|mut key| {
        key.legacy_dlights = false;
        key
    });
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
    K: Sync,
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
            let job_started = Instant::now();
            let value = create(job);
            done.push((index, value, job_started.elapsed()));
        }
        done
    };
    let mut finished = if workers <= 1 {
        run_worker()
    } else {
        thread::scope(|scope| {
            let handles = (0..workers)
                .map(|_| scope.spawn(&run_worker))
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
            )
        },
    );
    jobs.into_iter().zip(compiled).collect()
}

pub(in crate::renderer) fn ocean_pipeline_key() -> PipelineKey {
    PipelineKey {
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
            )
        },
    );
    jobs.into_iter().zip(compiled).collect()
}

pub(in crate::renderer) fn create_depth_prepass_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA AO depth prepass pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            }],
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
            entry_point: Some("fs_main"),
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
        cache: None,
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
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            }],
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
        cache: None,
    })
}

pub(in crate::renderer) fn create_depth_prepass_mask_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA alpha-tested depth prepass pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_mask"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            }],
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
            entry_point: Some("fs_mask"),
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
        cache: None,
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
            buffers: &[vertex_layout],
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
        cache: None,
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
            buffers: &[vertex_layout],
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
        cache: None,
    })
}
