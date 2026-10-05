//! Frame plan.
use crate::renderer::{
    weather, CullDebugMode, DetailTextureMode, DynamicLightsMode, DynamicShadowsMode,
    PlanarReflectionDebugMode, Renderer, WorldRenderPath,
};

impl Renderer {
    pub(in crate::renderer) fn rebuild_frame_plan(&mut self) {
        let fog_enabled = self.weather.fog.volumetric_effective();
        let legacy_fog_enabled = self.weather.fog.legacy_effective();
        let legacy1_global_post = self.weather.fog.legacy1_global_post_active();
        let gamma_enabled = (self.output_gamma() - 1.0).abs() > 0.001;
        let lut_enabled = self.color_lut_effective_strength > 0.001;
        let full_post_effects = self.ocean_enabled
            || self.hdr_enabled
            || self.tone_mapping_enabled
            || self.bloom_enabled
            || self.halation_enabled
            || self.ssao_enabled
            || self.fxaa_enabled
            || self.taa_enabled
            || self.contact_shadows_enabled
            || fog_enabled
            || legacy1_global_post
            || self.clouds_enabled
            || self.weather.rain.enabled
            || self.weather.rain.puddle_amount > weather::RAIN_PUDDLE_EPSILON
            || self.ssr_enabled
            || self.reflection_debug_enabled
            || self.chromatic_aberration_strength > 0.001
            || self.film_grain_strength > 0.001
            || self.motion_blur_strength > 0.001
            || self.depth_of_field_strength > 0.001;
        // SMAA is a separate reference-quality resolve after the renderer's
        // chosen output path. It does not by itself require our post-process
        // intermediate: with gamma/effects off the world can render directly
        // into the SMAA color target before SMAA resolves to the surface.
        let use_post = gamma_enabled || lut_enabled || full_post_effects;
        // Gamma and color LUTs affect only the final image. Keep the fast BSP
        // shaders/frame recorder and reuse one compact post pass for both.
        let use_hiz = self.gpu_driven_enabled && self.hiz_occlusion_enabled;

        // Render settings are resolved here, on the cold path. The maximum-FPS
        // frame loop should not rediscover that every advanced feature is OFF
        // hundreds of times per frame/per draw. This path intentionally matches
        // the feature envelope of the known ~2100 FPS renderer: basic BSP, PVS,
        // texture filtering, MSAA, optional wireframe, optional gamma, and UI.
        let fast_baseline = !self.force_unified_world
            && !self.hdr_enabled
            && !self.tone_mapping_enabled
            && !self.bloom_enabled
            && !self.halation_enabled
            && !self.ssao_enabled
            && !self.fxaa_enabled
            && !self.smaa_enabled
            && !self.taa_enabled
            && !self.contact_shadows_enabled
            && !fog_enabled
            && !legacy_fog_enabled
            && !self.clouds_enabled
            && !self.weather.rain.enabled
            && !self.ocean_enabled
            && self.weather.rain.surface_wetness <= weather::RAIN_WETNESS_EPSILON
            && self.weather.rain.puddle_amount <= weather::RAIN_PUDDLE_EPSILON
            && !self.ssr_enabled
            && !self.reflection_debug_enabled
            && self.chromatic_aberration_strength <= 0.001
            && !self.vignette_enabled
            && self.film_grain_strength <= 0.001
            && self.motion_blur_strength <= 0.001
            && self.depth_of_field_strength <= 0.001
            && !self.gpu_driven_enabled
            && !self.hiz_occlusion_enabled
            && self.detail_textures_mode == DetailTextureMode::Off
            && !self.point_lighting_enabled()
            && !matches!(
                self.dynamic_lights_mode,
                DynamicLightsMode::Legacy
                    | DynamicLightsMode::Vertex
                    | DynamicLightsMode::ClusteredLite
            )
            && !self.emissive_area_lights_enabled
            && !self.irradiance_volume_enabled
            && !self.voxel_probe_gi_enabled
            && !self.local_light_shadows_enabled
            && !self.pbr_enabled
            && !self.cascaded_shadows_enabled
            && !(self.hardware_rt_requested() && self.ray_traced_shadows.is_some())
            && self.cull_debug_mode == CullDebugMode::Off
            && !self.planar_reflection.active
            && self.planar_reflection_debug_mode == PlanarReflectionDebugMode::Off
            && !self.jump_shade.engaged();
        // Hi-Z reads the camera depth buffer, so it no longer forces the full
        // linear-depth prepass: with no other consumer it runs a cheaper
        // depth-only early pass instead (see `FramePlan::hiz_early`).
        let needs_linear_depth = self.ssao_enabled
            || (self.rt_reduced_shadows
                && self.cascaded_shadow_mode == DynamicShadowsMode::RayTraced
                && self.ray_tracing_supported)
            || self.taa_enabled
            || self.contact_shadows_enabled
            || fog_enabled
            || legacy1_global_post
            || self.clouds_enabled
            || self.weather.rain.enabled
            || self.ssr_enabled
            || self.reflection_debug_enabled
            || self.motion_blur_strength > 0.001
            || self.depth_of_field_strength > 0.001;
        self.frame_plan = FramePlan {
            world_path: if fast_baseline {
                WorldRenderPath::FastBaseline
            } else {
                WorldRenderPath::Advanced
            },
            use_post,
            lut_post: lut_enabled,
            // The tiny post path handles display gamma and/or one trilinear 3-D
            // LUT sample. If that path is already required it also applies the
            // vignette, but vignette alone does not force an offscreen scene copy.
            gamma_only_post: (gamma_enabled || lut_enabled) && !full_post_effects,
            needs_linear_depth,
            use_hiz,
            hiz_early: use_hiz && !needs_linear_depth,
            use_gpu_culling: self.gpu_driven_enabled || self.cull_debug_mode != CullDebugMode::Off,
            // Area lights use the same clustered lists as entity/point lights,
            // but remain independently toggleable. The active BSP pipeline is
            // specialized for the requested source classes when settings change.
            use_clustered_lighting: self.point_lighting_enabled()
                || self.dynamic_lights_mode == DynamicLightsMode::ClusteredLite
                || self.emissive_area_lights_enabled,
        };
        let plan = self.frame_plan;
        self.ensure_post_pipeline(plan.use_post && !plan.gamma_only_post);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::renderer) struct FramePlan {
    pub(in crate::renderer) world_path: WorldRenderPath,
    pub(in crate::renderer) use_post: bool,
    /// Select the already-compiled compact grading shader on the fast path.
    /// Color-only effects never require the advanced world renderer.
    pub(in crate::renderer) lut_post: bool,
    pub(in crate::renderer) gamma_only_post: bool,
    pub(in crate::renderer) needs_linear_depth: bool,
    pub(in crate::renderer) use_hiz: bool,
    /// Hi-Z is the only consumer of depth this frame, so no full prepass is
    /// drawn: a depth-only early pass of last frame's visible batches builds
    /// the pyramid instead (Bevy's two-phase occlusion culling, minus the late
    /// prepass nothing here would read).
    pub(in crate::renderer) hiz_early: bool,
    pub(in crate::renderer) use_gpu_culling: bool,
    pub(in crate::renderer) use_clustered_lighting: bool,
}
