//! Frame plan.
use crate::renderer::{
    weather, CullDebugMode, DofMode, DynamicLightsMode,
    PlanarReflectionDebugMode, Renderer, WorldRenderPath,
};

/// Defer truly translucent material stage chains after opaque/cloud drawing.
/// An authored opaque material can still contain blended later stages (terrain
/// splats, lightmaps, caustics). Those stages must finish BEFORE the cloud
/// multiplier, or they overwrite the shadowed material's opaque base.
/// The map-load cached material flag avoids a shader lookup per rendered stage.
/// The key matches same_world_surface, without borrowing GPU batches.
#[derive(Default)]
pub(super) struct WorldTransparencyOrder {
    surface: Option<(std::ops::Range<u32>, u32)>,
    deferred: bool,
}

impl WorldTransparencyOrder {
    /// Attached world decals must be in the optical background. Their blend
    /// class does not make them an independent translucent pane. Explicit late
    /// authoring still takes precedence. No change when glass is absent.
    pub(super) fn capture_decal(glass: bool, polygon_offset: bool, late_sorted: bool) -> bool {
        glass && polygon_offset && !late_sorted
    }
    pub(super) fn defer_stage(
        &mut self, vertices: &std::ops::Range<u32>, shader: u32,
        class: crate::scene::DrawClass, opaque_composite_before_clouds: bool,
        late_sorted_material: bool, early_sorted_material: bool,
    ) -> bool {
        let same_surface = self.surface.as_ref().is_some_and(|(previous, previous_shader)|
            previous == vertices && *previous_shader == shader);
        if !same_surface {
            self.surface = Some((vertices.clone(), shader));
            self.deferred = false;
        }
        // TaystJK sorts the *shader*, not individual stages. An explicit
        // late sort must defer the alpha-tested/opaque depth seed as well as
        // its blended siblings, preserving `depthFunc equal` overlays.
        // A shader sorted before SS_OPAQUE (portal mirrors) draws first in
        // TaystJK even when its stages blend; its depth seeds what follows.
        self.deferred |= late_sorted_material
            || (class == crate::scene::DrawClass::Transparent
                && !opaque_composite_before_clouds
                && !early_sorted_material);
        self.deferred
    }
}

#[cfg(test)]
mod transparency_order_tests {
    use super::WorldTransparencyOrder;
    use crate::scene::DrawClass::{Opaque, Transparent};

    #[test]
    fn glass_capture_includes_attached_decals_but_preserves_explicit_late_sort() {
        assert!(WorldTransparencyOrder::capture_decal(true, true, false));
        assert!(!WorldTransparencyOrder::capture_decal(false, true, false));
        assert!(!WorldTransparencyOrder::capture_decal(true, false, false));
        assert!(!WorldTransparencyOrder::capture_decal(true, true, true));
    }

    #[test]
    fn opaque_entities_render_before_water_without_reversing_later_shader_stages() {
        let mut order = WorldTransparencyOrder::default();
        let stages = [(0..3, 0, Opaque), (3..6, 1, Transparent),
            (3..6, 1, Opaque), (6..9, 2, Opaque)];
        let (mut before_entities, mut after_entities) = (Vec::new(), Vec::new());
        for (index, (vertices, shader, class)) in stages.iter().enumerate() {
            if order.defer_stage(vertices, *shader, *class, false, false, false) {
                after_entities.push(index);
            } else {
                before_entities.push(index);
            }
        }
        assert_eq!(before_entities, [0, 3]);
        assert_eq!(after_entities, [1, 2], "the replacement must remain after its blend");
    }

    #[test]
    fn portal_sorted_blended_stages_draw_before_opaque() {
        // A mirror floor (`portal`, SS_PORTAL) seeds depth from a blended
        // stage; deferring it would show geometry and entities beneath it.
        let mut order = WorldTransparencyOrder::default();
        assert!(!order.defer_stage(&(0..6), 7, Transparent, false, false, true));
        assert!(!order.defer_stage(&(0..6), 7, Transparent, false, false, true));
        assert!(order.defer_stage(&(6..12), 8, Transparent, false, false, false));
    }

    #[test]
    fn opaque_terrain_blend_lightmap_and_caustics_finish_before_clouds() {
        let mut order = WorldTransparencyOrder::default();
        // Mirrors sand -> wet sand -> lightmap -> caustics -> depthfade.
        for class in [Opaque, Transparent, Transparent, Transparent, Transparent] {
            assert!(!order.defer_stage(&(0..6), 109, class, true, false, false));
        }
        // Stock water is additive and remains behind the cloud multiplier.
        assert!(order.defer_stage(&(6..12), 102, Transparent, false, false, false));
        assert!(order.defer_stage(&(6..12), 102, Transparent, false, false, false));
    }

    #[test]
    fn additive_sorted_masked_depth_seed_waits_with_its_depth_equal_overlays() {
        use crate::scene::DrawClass::Mask;
        let mut order = WorldTransparencyOrder::default();
        // seph_seaweed2: masked GL_ONE GL_ZERO depth seed, followed by
        // additive caustics and a source-alpha depthfade overlay.
        let range = 18..198;
        assert!(order.defer_stage(&range, 114, Mask, false, true, false));
        for _ in 0..4 {
            assert!(order.defer_stage(&range, 114, Transparent, false, true, false));
        }
        // The next material must not inherit the late-sort decision.
        assert!(!order.defer_stage(&(198..204), 115, Opaque, false, false, false));
    }

    #[test]
    fn distinct_materials_do_not_inherit_another_surfaces_deferred_state() {
        let mut order = WorldTransparencyOrder::default();
        assert!(order.defer_stage(&(0..3), 0, Transparent, false, false, false));
        assert!(!order.defer_stage(&(0..3), 1, Opaque, false, false, false));
        assert!(order.defer_stage(&(0..3), 1, Transparent, false, false, false));
        assert!(!order.defer_stage(&(3..6), 1, Opaque, false, false, false));
    }
}

impl Renderer {
    /// DOF is drawn under this plan: `Off` never draws, underwater-only draws
    /// only while the camera is below a water surface (`dof_plan_underwater`).
    pub(in crate::renderer) fn dof_draws_this_plan(&self) -> bool {
        self.depth_of_field_strength > 0.001
            && match self.dof_mode {
                DofMode::Off => false,
                DofMode::On => true,
                DofMode::UnderwaterOnly => self.dof_plan_underwater,
            }
    }

    pub(in crate::renderer) fn rebuild_frame_plan(&mut self) {
        let fog_enabled = self.weather.fog.volumetric_effective();
        let legacy_fog_enabled = self.weather.fog.legacy_effective();
        let legacy1_global_post = self.weather.fog.legacy1_global_post_active();
        // Legacy GL kept r_drawfog 2 smooth with framebuffer dithering. Our
        // equivalent uses a float scene plus one dithered presentation pass so
        // the fog blend is not irreversibly quantized before presentation.
        let legacy2_precision_post = self.weather.fog.legacy2_precision_active();
        let gamma_enabled = (self.output_gamma() - 1.0).abs() > 0.001;
        let lut_enabled = self.color_lut_effective_strength > 0.001;
        let manual_exposure = self.color_grade.exposure_ev.abs() > 0.001;
        // HDR by itself only changes the scene target to Rgba16Float. It does
        // not need the heavyweight spatial post shader; the compact presentation
        // pass can resolve the float scene to the swapchain. Keep HDR out of the
        // full-post bucket so forced Unified/Advanced + r_hdr 1 pays only the
        // unavoidable 16F bandwidth and one tiny fullscreen resolve.
        let hdr_resolve = self.hdr_enabled || self.native_atmosphere_active()
            || self.blue_water_environment.weather_backend == 1;
        let rain_spatial_post = self.weather.rain.enabled && !self.weather.rain.fast_post;
        let rain_lens_copy = self.weather.rain.enabled
            && self.weather.rain.glass_enabled
            && self.weather.rain.fast_post;
        let other_full_post_effects = self.ocean_enabled
            || (self.sky_backend == 1 && self.blue_water_environment.filmic)
            || self.tone_mapping_enabled
            || self.glow_mode.enabled()
            || self.halation_enabled
            || self.ssao_enabled
            || self.fxaa_enabled
            || self.temporal_resolve_active()
            || self.contact_shadows_enabled
            || fog_enabled
            || legacy1_global_post
            || self.clouds_enabled
            || self.weather.snow.enabled
            || self.weather.atmosphere.active()
            || self.ssr_enabled
            || self.reflection_debug_enabled
            || self.chromatic_aberration_strength > 0.001
            || self.film_grain_strength > 0.001
            || self.motion_blur_strength > 0.001
            || self.dof_draws_this_plan();
        let rain_full_post_effects = rain_spatial_post
            || (self.weather.rain.puddle_amount > weather::RAIN_PUDDLE_EPSILON
                && !self.weather.rain.fast_post);
        let full_post_effects = other_full_post_effects || rain_full_post_effects;
        let rain_only_post = self.weather.rain.lean_post
            && rain_full_post_effects
            && !other_full_post_effects;
        // SMAA is a separate reference-quality resolve after the renderer's
        // chosen output path. It does not by itself require our post-process
        // intermediate: with gamma/effects off the world can render directly
        // into the SMAA color target before SMAA resolves to the surface.
        let compact_post_reason = hdr_resolve
            || gamma_enabled
            || lut_enabled
            || manual_exposure
            || legacy2_precision_post
            || rain_lens_copy;
        let use_post = compact_post_reason || full_post_effects;
        // Gamma and color LUTs affect only the final image. Keep the fast BSP
        // shaders/frame recorder and reuse one compact post pass for both.
        let use_hiz = self.gpu_driven_enabled && self.hiz_occlusion_enabled;

        // Render settings are resolved here, on the cold path. The maximum-FPS
        // frame loop should not rediscover that every advanced feature is OFF
        // hundreds of times per frame/per draw. This path intentionally matches
        // the feature envelope of the known ~2100 FPS renderer: basic BSP, PVS,
        // texture filtering, MSAA, optional wireframe, optional gamma, and UI.
        let fast_baseline = self.sky_backend == 0 && self.blue_water_environment.world_lighting <= 0.0 && self.blue_water_environment.weather_backend != 1 && !self.force_unified_world
            // bsp_fast has no baked-light adjustment or authored-sun cache pass.
            && !self.baked_lighting_shader_needed()
            // Curved-grid LOD belongs to the Unified packed BSP path.
            && self.world.as_ref().is_none_or(|world| !world.patch_lod.has_patches())
            && !self.dynamic_light_debug
            && !self.hdr_enabled
            && !self.tone_mapping_enabled
            && !self.glow_mode.enabled()
            && !self.halation_enabled
            && !self.ssao_enabled
            && !self.fxaa_enabled
            && !self.smaa_enabled
            && !self.temporal_resolve_active()
            && !self.contact_shadows_enabled
            && !fog_enabled
            && !legacy_fog_enabled
            && !self.clouds_enabled
            && !self.weather.rain.enabled
            && !self.weather.snow.enabled
            && !self.weather.atmosphere.active()
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
            && !self.detail_textures_enabled
            && !self.point_lighting_enabled()
            && !matches!(
                self.dynamic_lights_mode,
                DynamicLightsMode::Legacy
                    | DynamicLightsMode::Vertex
                    | DynamicLightsMode::ClusteredLite
            )
            && !self.any_area_lights_enabled()
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
        let other_linear_depth = self.ssao_enabled
            || (self.rt_sun_cache_wanted() || self.rt_local_cache_wanted())
            || self.temporal_resolve_active()
            || self.contact_shadows_enabled
            || fog_enabled
            || legacy1_global_post
            || self.clouds_enabled
            || self.weather.snow.enabled
            || self.weather.atmosphere.active()
            || self.ssr_enabled
            || self.reflection_debug_enabled
            || self.motion_blur_strength > 0.001
            || self.dof_draws_this_plan();
        let needs_linear_depth = other_linear_depth || rain_spatial_post;
        // Which consumers asked for the full prepass (bit order matches the
        // `other_linear_depth` terms, then rain); recorded by render_perf_capture.
        let linear_depth_reasons = [
            self.ssao_enabled,
            self.rt_sun_cache_wanted() || self.rt_local_cache_wanted(),
            self.temporal_resolve_active(),
            self.contact_shadows_enabled,
            fog_enabled,
            legacy1_global_post,
            self.clouds_enabled,
            self.weather.snow.enabled,
            self.weather.atmosphere.active(),
            self.ssr_enabled,
            self.reflection_debug_enabled,
            self.motion_blur_strength > 0.001,
            self.dof_draws_this_plan(),
            rain_spatial_post,
        ].iter().enumerate().fold(0u32, |bits, (bit, &on)| bits | (u32::from(on) << bit));
        let rain_only_linear_depth = rain_spatial_post && !other_linear_depth;
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
            // Legacy 2 only needs a precision-preserving copy plus final dither;
            // do not compile/run the heavyweight full-post shader just for fog.
            // Despite the historical field name, this is the compact color-only
            // presentation path. HDR-only belongs here too: the world remains on
            // the selected (including forced Unified/Advanced) path, but we avoid
            // the full depth-aware post shader when no effect actually needs it.
            gamma_only_post: compact_post_reason && !full_post_effects,
            needs_linear_depth,
            linear_depth_reasons,
            rain_only_linear_depth,
            rain_only_post,
            use_hiz,
            hiz_early: use_hiz && !needs_linear_depth,
            use_gpu_culling: self.gpu_driven_enabled || self.cull_debug_mode != CullDebugMode::Off,
            // Area lights use the same clustered lists as entity/point lights,
            // but remain independently toggleable. The active BSP pipeline is
            // specialized for the requested source classes when settings change.
            use_clustered_lighting: self.point_lighting_enabled()
                || self.dynamic_lights_mode == DynamicLightsMode::ClusteredLite
                || self.any_area_lights_enabled(),
        };
        let plan = self.frame_plan;
        // Underwater-only DOF: compile the full post PSO up front so the first
        // dive does not wait for it.
        let dof_may_need_post = self.depth_of_field_strength > 0.001 && self.dof_mode == DofMode::UnderwaterOnly;
        self.ensure_post_pipeline(
            (plan.use_post && !plan.gamma_only_post && !plan.rain_only_post) || dof_may_need_post,
        );
        self.ensure_rain_post_pipeline(plan.rain_only_post);
        self.ensure_cloud_shadow_opaque_pipeline();
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
    /// Bitmask of the consumers behind `needs_linear_depth`: 0 SSAO, 1 RT reduced
    /// shadows, 2 temporal resolve, 3 contact shadows, 4 fog, 5 legacy global
    /// post, 6 clouds, 7 snow, 8 atmosphere, 9 SSR, 10 reflection debug,
    /// 11 motion blur, 12 depth of field, 13 rain spatial post.
    pub(in crate::renderer) linear_depth_reasons: u32,
    /// Rain is the sole consumer of the full-resolution linear-depth prepass.
    pub(in crate::renderer) rain_only_linear_depth: bool,
    /// Rain is the only spatial post effect and the rain-only PSO is requested.
    pub(in crate::renderer) rain_only_post: bool,
    pub(in crate::renderer) use_hiz: bool,
    /// Hi-Z is the only consumer of depth this frame, so no full prepass is
    /// drawn: a depth-only early pass of last frame's visible batches builds
    /// the pyramid instead (Bevy's two-phase occlusion culling, minus the late
    /// prepass nothing here would read).
    pub(in crate::renderer) hiz_early: bool,
    pub(in crate::renderer) use_gpu_culling: bool,
    pub(in crate::renderer) use_clustered_lighting: bool,
}
