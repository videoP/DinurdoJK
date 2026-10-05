//! Settings video.
use crate::ui::{
    CloudRenderResolution, CloudType, ColorLutPreset, CullDebugMode, DetailTextureMode, DofQuality,
    DynamicLightsMode, DynamicShadowsMode, EntityAmbientLightingMode, EntityShadowLight, FogMode,
    FootprintMode, FullscreenMode, FxGeometryMode, Ghoul2BatchMode, Ghoul2SkinningMode,
    PlanarReflectionDebugMode, PuddleQuality, PvsMode, RainIntensity, ReflectionQuality,
    RendererBackend, SaberMarkMode, SunVisibilityMode, TextureFilter, VsyncMode,
};

#[derive(Debug, Clone, Copy)]
pub struct VideoSettings {
    pub fullscreen: FullscreenMode,
    pub renderer_backend: RendererBackend,
    pub vsync: VsyncMode,
    /// Maximum number of frames the WGPU presentation surface may keep in flight.
    /// Lower values favor latency; higher values favor throughput.
    pub max_frame_latency: u32,
    pub msaa_samples: u32,
    pub resolution: [u32; 2],
    pub window_position: Option<[i32; 2]>,
    pub window_maximized: bool,
    pub texture_filter: TextureFilter,
    /// Classic `r_picmip`: number of highest map-texture mip levels omitted
    /// when the texture is uploaded. TaystJK defaults to 0.
    pub picmip: u32,
    pub detail_textures: DetailTextureMode,
    /// Port of the standard distance-based detail fade: blend the detail contribution
    /// back to its neutral identity as camera distance approaches the configured range.
    pub detail_texture_fade: bool,
    pub detail_texture_fade_distance: f32,
    pub wireframe_mask: u32,
    pub skip_ui: bool,
    pub pvs_mode: PvsMode,
    pub fps_cap: u32,
    /// Continuous projectile FX sampling rate. `0` preserves legacy JKA's
    /// render-frame-driven behavior; non-zero values are fixed Hz.
    pub fx_fps: u32,
    /// `cg_fxFPSScope`: 0 continuous EFX only, 1 also stock frame-driven FX.
    pub fx_fps_scope: u32,
    /// `fx_physics`: 0 off, 2 authored expensivePhysics (default), 3 force all.
    pub fx_physics: u32,
    /// `fx_lod`: 0 stock, 1 authored cullRange, 2 adaptive screen-size density.
    pub fx_lod: u32,
    /// Stock `fx_countScale` (0..1).
    pub fx_count_scale: f32,
    /// `r_fxLodScale`: multiplies authored EFX cullRange (default 5, like r_lodscale).
    pub fx_lod_scale: f32,
    /// `r_lodScale`: Ghoul2 projected-size LOD scale (OpenJK default 5).
    pub lod_scale: f32,
    /// View-dependent EFX geometry expansion path. FX simulation itself already
    /// runs on the dedicated `jka-fx` worker; this controls the later sprite/line
    /// tessellation step against the final camera.
    pub fx_geometry: FxGeometryMode,
    /// A/B diagnostic for GPU-instanced EFX sprites. When enabled, fragments
    /// whose final source alpha is exactly zero are discarded before fog/blend.
    /// This preserves non-zero-alpha edges and is intentionally limited to
    /// source-alpha sprite blend modes.
    pub fx_zero_alpha_discard: bool,
    /// Server-placed MD3 map props (misc_model_* / func_static models). Inline
    /// BSP brush models and geometry compiled into the BSP are unaffected.
    pub draw_map_models: bool,
    /// Debug overlays for trigger volumes and clip/collision-only brushes.
    /// Session-only: never written to the config.
    pub draw_triggers: bool,
    pub draw_clip_brushes: bool,
    /// `r_drawEntities`: NetRadiant-style colored boxes, classname labels and
    /// target/targetname link lines for every map entity. Session-only.
    pub draw_entities: bool,
    pub draw_fps: u8,
    /// `cg_drawTimer`: classic elapsed level timer (M:SS only).
    pub draw_timer: bool,
    /// `developer`: 0 quiet, 1 basic diagnostics, 2 verbose, 3 trace.
    /// `developer_tools` remains the derived compatibility gate used by
    /// inspector/overlay code: it is always `developer_level != 0`.
    pub developer_level: u8,
    pub developer_tools: bool,
    /// Classic renderer-only verbose spew level (`r_verbose`).
    pub renderer_verbose: u8,
    pub perf_trace: bool,
    /// World renderer choice (`r_worldPath unified`): keeps the world on
    /// the unified renderer even when the FastBaseline feature envelope matches,
    /// so "minimal unified" can be timed against the untouched baseline.
    pub force_unified_world: bool,
    /// Session-only: parallax occlusion on top of `r_pbr` (`r_pom`, default on).
    pub pom: bool,
    pub gpu_timings: bool,
    pub ghoul2_skinning: Ghoul2SkinningMode,
    pub ghoul2_early_cull: bool,
    pub ghoul2_lod_bias: i32,
    pub ghoul2_batch_draws: Ghoul2BatchMode,
    /// `r_ghoul2animsmooth`: jaPRO `CBoneCache::SmoothLow` renderer bone-history
    /// filter factor. Matches jaPRO's own default of 0.3; only active strictly
    /// between 0 and 1 (0 or >=1 disables it).
    pub ghoul2_anim_smooth: f32,
    pub physics_msec: u32,
    /// Request a 1 ms Windows multimedia timer period for timeout/sleep waits.
    /// Raw mouse events already wake the event loop independently.
    pub timer_resolution_1ms: bool,
    /// Experimental render-thread late latch. Subframe input is always enabled;
    /// the renderer resamples the newest real view orientation at its last
    /// coherent camera point for the active path.
    pub input_latelatch: bool,
    // Client-side visual physics (Rapier integration target). These never replace
    // authoritative JKA/OpenJK player movement or server entity state.
    pub client_physics: bool,
    pub client_physics_hz: u32,
    pub client_physics_max_substeps: u32,
    pub client_physics_ccd: bool,
    pub client_physics_sleeping: bool,
    pub ragdolls: bool,
    pub ragdoll_max: u32,
    pub ragdoll_lifetime: f32,
    pub ragdoll_self_collision: bool,
    /// Profile-driven post-Ghoul2 soft-tissue secondary motion.
    pub jiggle_physics: bool,
    /// 0 = KawaiiPhysics-derived point solver, 1 = naelstrof/JigglePhysics Verlet solver.
    pub jiggle_solver: u8,
    pub jiggle_strength: f32,
    pub jiggle_breast_strength: f32,
    pub jiggle_glute_strength: f32,
    pub jiggle_stiffness: f32,
    pub jiggle_damping: f32,
    pub jiggle_glute_lift: f32,
    pub jiggle_jp_stiffness: f32,
    pub jiggle_jp_drag: f32,
    pub jiggle_jp_air_drag: f32,
    pub jiggle_jp_stretch: f32,
    pub jiggle_jp_soften: f32,
    pub jiggle_jp_gravity: f32,
    /// OpenJK cg_dismember: 0 off, 1 limbs only, 2 full.
    pub dismemberment: u8,
    pub dismember_max: u32,
    pub dismember_lifetime: f32,
    /// Experimental Ghoul2 cape/cloak/robe presentation cloth.
    pub cloth_physics: bool,
    pub cloth_body_collision: bool,
    pub cloth_body_clearance: f32,
    pub cloth_air_resistance: f32,
    pub cloth_turn_response: f32,
    pub cloth_animation_influence: f32,
    pub cloth_wind: bool,
    pub physics_props: bool,
    pub physics_prop_max: u32,
    pub physics_debris: bool,
    pub physics_debris_max: u32,
    pub physics_debris_lifetime: f32,
    pub physics_player_push: bool,
    pub physics_weapon_impulses: bool,
    pub physics_explosion_impulses: bool,
    pub physics_force_impulses: bool,
    pub physics_debug_draw: bool,
    pub physics_stats: bool,
    pub gamma: f32,
    pub gamma_method: crate::gamma::GammaMethod,
    /// Model lighting brightness on the same scale as `gamma`. While
    /// `model_brightness_locked` it tracks the master slider and adds nothing.
    pub model_brightness: f32,
    pub model_brightness_locked: bool,
    /// Dynamic (runtime) light brightness, same scale and lock rules.
    pub dynamic_light_brightness: f32,
    pub dynamic_light_brightness_locked: bool,
    pub hdr: bool,
    pub float_lightmap: bool,
    pub tone_mapping: bool,
    pub auto_exposure: bool,
    pub bloom: bool,
    pub halation: bool,
    pub ssao: bool,
    pub static_bsp_ao: bool,
    /// false = cached per-vertex AO, true = cached lightmap-space AO.
    pub static_bsp_ao_lightmap: bool,
    pub static_bsp_ao_samples: u32,
    /// Odd supersampling scales preserve the original lightmap sample lattice.
    pub static_bsp_ao_resolution: u32,
    /// Baked AO blend strength, percentage (25..100).
    pub static_bsp_ao_strength: u32,
    /// Baked AO distance scale, percentage of the 32/128/512-unit defaults.
    pub static_bsp_ao_range: u32,
    /// Debug/testing mode: bake only the nearest AO lightmap tile to the camera.
    pub static_bsp_ao_current_cell: bool,
    pub fxaa: bool,
    pub smaa: bool,
    pub taa: bool,
    pub contact_shadows: bool,
    pub fog_mode: FogMode,
    pub fog_strength: f32,
    /// When false, use the strongest q3map-authored sky sun from the current map.
    pub sun_override: bool,
    /// q3map azimuth in degrees: 0=east, 90=north.
    pub sun_yaw: f32,
    /// q3map elevation in degrees above the horizon.
    pub sun_pitch: f32,
    pub sun_intensity: f32,
    /// Unnormalized RGB chosen by the user; renderer normalizes it like q3map.
    pub sun_color: [f32; 3],
    /// Controls whether direct shader-sun light requires an authored sky portal.
    pub sun_visibility: SunVisibilityMode,
    /// Strips the baked map sun out of the entity lightgrid sample and re-adds
    /// it as a directional light that follows the runtime sun (color, intensity,
    /// direction). Needs Entity ambient lighting = BSP lightgrid.
    pub entity_sun_lighting: bool,
    /// 0 = map/default distanceCull; otherwise multiplier applied to that base.
    pub distance_cull_scale: f32,
    pub clouds: bool,
    pub cloud_type: CloudType,
    pub cloud_quality: f32,
    pub cloud_coverage: f32,
    pub cloud_height: f32,
    pub cloud_thickness: f32,
    pub cloud_shadows: bool,
    /// One authoritative atmospheric wind shared by clouds, precipitation, grass, and ocean.
    pub weather_wind: crate::ocean::OceanWind,
    pub rain: bool,
    pub rain_intensity: RainIntensity,
    pub puddle_quality: PuddleQuality,
    /// How readily rain collects in scattered puddles on large flat ground, 0..1.
    pub puddle_scatter: f32,
    /// Strength of the wet-weather colour grade (cool shadows, warm highlights,
    /// richer neon) while it rains, 0..1.
    pub rain_grade: f32,
    pub footprints: FootprintMode,
    pub grass: bool,
    /// Runtime A/B diagnostic: hoist root wind/clump work to a compute pass.
    pub grass_precompute: bool,
    /// Runtime A/B diagnostic: use the 5-triangle middle blade LOD.
    pub grass_mid_lod: bool,
    /// Runtime A/B diagnostic: order opaque grass near-to-far for early-Z.
    pub grass_front_to_back: bool,
    pub contact_shadow_debug: u8,
    pub ocean: bool,
    pub ocean_settings: crate::ocean::OceanSettings,
    pub cloud_render_resolution: CloudRenderResolution,
    pub cloud_temporal: bool,
    pub cloud_temporal_depth_fix: bool,
    pub cloud_shear: f32,
    pub cloud_base_variation: f32,
    pub cloud_shape_evolution: bool,
    pub cloud_terrain_interaction: bool,
    pub cloud_empty_skip: bool,
    pub cloud_aerial: f32,
    pub cloud_sky_ambient: bool,
    pub cloud_history_blend: f32,
    pub cloud_motion_reject: f32,
    pub cloud_history_depth_reject: bool,
    pub cloud_thickness_variation: f32,
    pub cloud_size: f32,
    /// Player-facing source of truth for reflection technique budgets.
    pub reflection_quality: ReflectionQuality,
    /// Development overlay showing the resolver result per visible surface/pixel.
    pub reflection_debug: bool,
    pub chromatic_aberration: f32,
    pub vignette: bool,
    pub film_grain_strength: f32,
    pub motion_blur_strength: f32,
    pub depth_of_field_strength: f32,
    /// Crosshair-surface autofocus for DOF. When off, the last focus distance is held.
    pub dof_autofocus: bool,
    pub dof_quality: DofQuality,
    pub color_lut: ColorLutPreset,
    pub color_lut_strength: f32,
    pub split_toning: crate::color_grading::SplitToningSettings,
    pub gpu_driven: bool,
    pub hiz_occlusion: bool,
    pub entity_ambient_lighting: EntityAmbientLightingMode,
    pub dynamic_lights: DynamicLightsMode,
    pub rt_samples: u32,
    /// 0 = stock JKA 1 - d^2/r^2, 1 = windowed inverse-square.
    pub dynamic_light_falloff: u32,
    pub rt_half_resolution: bool,
    /// Source `.map` only: approximate a compiled lighting pass from authored light entities.
    pub map_light_simulation: bool,
    /// Classic world-lighting master. False corresponds to vanilla r_fullbright 1.
    pub world_lighting: bool,
    /// Use BSP vertex colors instead of baked lightmaps for world static lighting.
    pub vertex_lighting: bool,
    /// Debug view equivalent to vanilla r_lightmap: show baked lighting without diffuse textures.
    pub lightmap_only: bool,
    /// Optional continuous-ribbon saber presentation. False keeps the OpenJK-style
    /// RT_SABER_GLOW sprite chain + RT_LINE core.
    pub modern_sabers: bool,
    /// Screen-space flare overlays such as the OpenJK saber clash flash.
    /// This does not disable the underlying impact FX, sounds, or marks.
    pub flares: bool,
    /// Authored saber-hit/block EFX presentation. The FX system retains tagged
    /// live primitives so this can be A/B toggled on a paused demo frame.
    pub saber_impact_fx: bool,
    /// Saber/world contact presentation. Legacy mirrors OpenJK's sparks, burn/glow
    /// marks and contact sound; Enhanced adds a hotter molten pass and drips.
    pub saber_marks: SaberMarkMode,
    /// Master switch for the complete Rend2-style PBR material profile.
    pub pbr: bool,
    /// Scope base-color replacements owned by PBR-enhanced MTR materials to
    /// PBR map preparation instead of letting them override classic rendering.
    pub allow_asset_overrides: bool,
    /// Generate a fallback normal map from diffuse luminance when no authored normal map exists.
    pub gen_normal_maps: bool,
    /// Use q3map2/Rend2 directional lightmaps when available; falls back to the BSP lightgrid.
    pub deluxe_mapping: bool,
    /// Scale the specular response produced by directional baked lighting.
    pub deluxe_specular: f32,
    pub emissive_area_lights: bool,
    pub voxel_probe_gi: bool,
    pub dynamic_shadows: DynamicShadowsMode,
    pub entity_shadow_light: EntityShadowLight,
    pub local_light_shadows: bool,
    pub cascaded_shadows: bool,
    pub cull_debug: CullDebugMode,
    pub planar_reflection_debug: PlanarReflectionDebugMode,
}

impl VideoSettings {
    /// Extra linear multiplier for model lighting. Locked (the default) leaves the
    /// master gamma as the only brightness control; unlocked, the slider value is
    /// expressed on the master's scale, so unlocking never changes the picture.
    pub fn effective_model_brightness(&self) -> f32 {
        Self::relative_brightness(
            self.model_brightness_locked,
            self.model_brightness,
            self.gamma,
        )
    }

    /// Same rule as [`Self::effective_model_brightness`] for runtime dynamic lights.
    pub fn effective_dynamic_light_brightness(&self) -> f32 {
        Self::relative_brightness(
            self.dynamic_light_brightness_locked,
            self.dynamic_light_brightness,
            self.gamma,
        )
    }

    pub(in crate::ui) fn relative_brightness(locked: bool, value: f32, gamma: f32) -> f32 {
        if locked {
            1.0
        } else {
            (value / gamma.max(0.01)).clamp(0.0, 6.0)
        }
    }

    /// Master slider moved: linked brightness sliders follow it.
    pub fn sync_linked_brightness(&mut self) {
        if self.model_brightness_locked {
            self.model_brightness = self.gamma;
        }
        if self.dynamic_light_brightness_locked {
            self.dynamic_light_brightness = self.gamma;
        }
    }
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            fullscreen: FullscreenMode::Windowed,
            renderer_backend: RendererBackend::Vulkan,
            vsync: VsyncMode::Off,
            max_frame_latency: 3,
            msaa_samples: 1,
            resolution: [1280, 800],
            window_position: None,
            window_maximized: false,
            texture_filter: TextureFilter::Trilinear,
            picmip: 0,
            detail_textures: DetailTextureMode::Off,
            detail_texture_fade: false,
            detail_texture_fade_distance: 512.0,
            wireframe_mask: 0,
            skip_ui: false,
            pvs_mode: PvsMode::Auto,
            fps_cap: 0,
            fx_fps: crate::fx::FX_FPS_DEFAULT,
            fx_fps_scope: crate::fx::FX_FPS_SCOPE_DEFAULT,
            fx_physics: crate::fx::FX_PHYSICS_DEFAULT,
            fx_lod: crate::fx::FX_LOD_DEFAULT,
            fx_count_scale: 1.0,
            fx_lod_scale: crate::fx::LOD_SCALE_DEFAULT,
            lod_scale: crate::fx::LOD_SCALE_DEFAULT,
            fx_geometry: FxGeometryMode::Cpu,
            fx_zero_alpha_discard: false,
            draw_map_models: true,
            draw_triggers: false,
            draw_clip_brushes: false,
            draw_entities: false,
            draw_fps: 1,
            draw_timer: false,
            developer_level: 0,
            developer_tools: false,
            renderer_verbose: 0,
            perf_trace: false,
            force_unified_world: false,
            pom: true,
            gpu_timings: false,
            ghoul2_skinning: Ghoul2SkinningMode::Gpu,
            ghoul2_early_cull: true,
            ghoul2_lod_bias: 0,
            ghoul2_batch_draws: Ghoul2BatchMode::Adaptive,
            ghoul2_anim_smooth: 0.3,
            physics_msec: 8,
            timer_resolution_1ms: false,
            input_latelatch: false,
            client_physics: false,
            client_physics_hz: 60,
            client_physics_max_substeps: 4,
            client_physics_ccd: true,
            client_physics_sleeping: true,
            ragdolls: true,
            ragdoll_max: 8,
            ragdoll_lifetime: 20.0,
            ragdoll_self_collision: false,
            jiggle_physics: false,
            jiggle_solver: 0,
            jiggle_strength: 1.0,
            jiggle_breast_strength: 1.0,
            jiggle_glute_strength: 1.0,
            jiggle_stiffness: 1.0,
            jiggle_damping: 1.0,
            jiggle_glute_lift: 0.0,
            // Initial body-region preset for naelstrof/JigglePhysics semantics.
            // Gravity intentionally starts at zero for soft tissue; raise it for
            // deliberate sag while the solver controls remain upstream-shaped.
            jiggle_jp_stiffness: 0.4,
            jiggle_jp_drag: 0.4,
            jiggle_jp_air_drag: 0.1,
            jiggle_jp_stretch: 0.8,
            jiggle_jp_soften: 0.0,
            jiggle_jp_gravity: 0.0,
            dismemberment: 0,
            dismember_max: 24,
            dismember_lifetime: 16.0,
            cloth_physics: false,
            cloth_body_collision: true,
            cloth_body_clearance: 1.0,
            cloth_air_resistance: 1.0,
            cloth_turn_response: 1.8,
            cloth_animation_influence: 0.35,
            cloth_wind: false,
            physics_props: true,
            physics_prop_max: 96,
            physics_debris: true,
            physics_debris_max: 192,
            physics_debris_lifetime: 10.0,
            physics_player_push: true,
            physics_weapon_impulses: true,
            physics_explosion_impulses: true,
            physics_force_impulses: true,
            physics_debug_draw: false,
            physics_stats: false,
            gamma: 1.0,
            gamma_method: crate::gamma::GammaMethod::Shader,
            model_brightness: 1.0,
            model_brightness_locked: true,
            dynamic_light_brightness: 1.0,
            dynamic_light_brightness_locked: true,
            hdr: false,
            float_lightmap: false,
            tone_mapping: false,
            auto_exposure: false,
            bloom: false,
            halation: false,
            ssao: false,
            static_bsp_ao: false,
            static_bsp_ao_lightmap: true,
            static_bsp_ao_samples: 32,
            static_bsp_ao_resolution: 3,
            static_bsp_ao_strength: 75,
            static_bsp_ao_range: 100,
            static_bsp_ao_current_cell: false,
            fxaa: false,
            smaa: false,
            taa: false,
            contact_shadows: false,
            fog_mode: FogMode::Off,
            fog_strength: 0.0,
            sun_override: false,
            // Matches the renderer's legacy fallback direction when a map has no authored sun.
            sun_yaw: 314.25595,
            sun_pitch: 57.04725,
            sun_intensity: 250.0,
            sun_color: [1.0, 1.0, 1.0],
            sun_visibility: SunVisibilityMode::SkyPortals,
            entity_sun_lighting: false,
            distance_cull_scale: 0.0,
            clouds: true,
            cloud_type: CloudType::Storm,
            cloud_quality: 1.0,
            cloud_coverage: 0.6,
            cloud_height: 4600.0,
            cloud_thickness: 1200.0,
            cloud_shadows: true,
            weather_wind: crate::ocean::OceanWind {
                speed: 167.0,
                direction: 220.0,
                gust: 0.2,
                shift: 0.0,
            },
            rain: false,
            rain_intensity: RainIntensity::Rain,
            puddle_quality: PuddleQuality::High,
            puddle_scatter: 0.8,
            rain_grade: 0.5,
            footprints: FootprintMode::ThreeD,
            grass: true,
            grass_precompute: true,
            grass_mid_lod: true,
            grass_front_to_back: true,
            contact_shadow_debug: 0,
            ocean: false,
            ocean_settings: crate::ocean::OceanSettings::default(),
            cloud_render_resolution: CloudRenderResolution::Full,
            cloud_temporal: true,
            cloud_temporal_depth_fix: true,
            cloud_shear: 0.2,
            cloud_base_variation: 1.0,
            cloud_shape_evolution: false,
            cloud_terrain_interaction: false,
            cloud_empty_skip: false,
            cloud_aerial: 0.0,
            cloud_sky_ambient: false,
            cloud_history_blend: 0.85,
            cloud_motion_reject: 1.0,
            cloud_history_depth_reject: false,
            cloud_thickness_variation: 0.75,
            cloud_size: 0.333,
            reflection_quality: ReflectionQuality::High,
            reflection_debug: false,
            chromatic_aberration: 0.0,
            vignette: false,
            film_grain_strength: 0.0,
            motion_blur_strength: 0.0,
            depth_of_field_strength: 0.0,
            dof_autofocus: true,
            dof_quality: DofQuality::Adaptive,
            color_lut: ColorLutPreset::Off,
            color_lut_strength: 1.0,
            split_toning: Default::default(),
            gpu_driven: false,
            hiz_occlusion: false,
            entity_ambient_lighting: EntityAmbientLightingMode::Off,
            dynamic_lights: DynamicLightsMode::Off,
            rt_samples: 1,
            dynamic_light_falloff: 0,
            rt_half_resolution: false,
            map_light_simulation: false,
            world_lighting: true,
            vertex_lighting: false,
            lightmap_only: false,
            modern_sabers: false,
            flares: true,
            saber_impact_fx: true,
            saber_marks: SaberMarkMode::Legacy,
            pbr: true,
            allow_asset_overrides: true,
            gen_normal_maps: false,
            deluxe_mapping: true,
            deluxe_specular: 1.0,
            emissive_area_lights: false,
            voxel_probe_gi: false,
            dynamic_shadows: DynamicShadowsMode::Off,
            entity_shadow_light: EntityShadowLight::Lightgrid,
            local_light_shadows: false,
            cascaded_shadows: false,
            cull_debug: CullDebugMode::Off,
            planar_reflection_debug: PlanarReflectionDebugMode::Off,
        }
    }
}
