//! Settings video.
use crate::ui::{
    CloudRenderResolution, CloudType, ColorLutPreset, CullDebugMode, DofMode, DofQuality,
    DynamicLightsMode, DynamicShadowsMode, EntityAmbientLightingMode, EntityShadowLight, FogMode,
    FootprintMode, FullscreenMode, FxGeometryMode, Ghoul2BatchMode, Ghoul2SkinningMode,
    CrtMask, CrtQuality, CrtReferenceResolution, CrtScope, CrtStyle, PlanarReflectionDebugMode,
    GlowMode, PuddleQuality, PvsMode, RainIntensity, ReflectionQuality, RendererBackend, SaberMarkMode,
    SunVisibilityMode, TextureFilter, VsyncMode,
};

#[derive(Debug, Clone, Copy)]
pub struct VideoSettings {
    pub patch_subdivisions: f32,
    pub patch_stitching: bool,
    pub lod_curve_error: f32,
    pub no_curves: bool,
    pub speeds: i32,
    pub fullscreen: FullscreenMode,
    pub renderer_backend: RendererBackend,
    /// 0 Auto, 1 discrete, 2 integrated, 3 CPU/software (if WGPU exposes it).
    pub gpu_adapter: u8,
    pub vsync: VsyncMode,
    /// Maximum number of frames the WGPU presentation surface may keep in flight.
    /// Lower values favor latency; higher values favor throughput.
    pub max_frame_latency: u32,
    /// Render the frame offscreen and submit it before acquiring the swapchain
    /// image, then copy (r_earlySubmit). Session-only.
    pub early_submit: bool,
    /// Render offscreen and copy into the swapchain image (r_presentCopy). Session-only.
    pub present_copy: bool,
    /// r_ext_compress_textures: 0 off, 1 BC1/BC3 like jamp, 2 also lightmaps.
    /// Latched to the next map load.
    pub texture_compression: u8,
    pub msaa_samples: u32,
    pub resolution: [u32; 2],
    pub window_position: Option<[i32; 2]>,
    pub window_maximized: bool,
    pub texture_filter: TextureFilter,
    /// Classic `r_picmip`: number of highest map-texture mip levels omitted
    /// when the texture is uploaded. TaystJK defaults to 0.
    pub picmip: u32,
    pub detail_textures: bool,
    /// Port of the standard distance-based detail fade: blend the detail contribution
    /// back to its neutral identity as camera distance approaches the configured range.
    pub detail_texture_fade: bool,
    pub detail_texture_fade_distance: f32,
    pub wireframe_mask: u32,
    /// Opt-in BSP material cost heatmap. No work while disabled.
    pub shader_complexity: bool,
    pub skip_ui: bool,
    /// TaystJK cg_draw2D: main gameplay 2D (HUD, scoreboards, overlays).
    /// Engine console, chat input and menus remain usable when disabled.
    pub draw_2d: bool,
    /// TaystJK cg_drawHud: core health/shield/ammo/force HUD, not movement tools.
    pub draw_hud: bool,
    pub pvs_mode: PvsMode,
    pub face_plane_cull: bool,
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
    /// `r_lodScale`: Ghoul2 and MD3 projected-size LOD scale (OpenJK default 5).
    pub lod_scale: f32,
    /// View-dependent EFX geometry expansion path. FX simulation itself already
    /// runs on the dedicated `jka-fx` worker; this controls the later sprite/line
    /// tessellation step against the final camera.
    pub fx_geometry: FxGeometryMode,
    /// Opt-in GPU-resident EFX sprite motion; requires r_fxGeometry gpu.
    pub fx_particle_simulation: bool,
    /// A/B diagnostic for GPU-instanced EFX sprites. When enabled, fragments
    /// whose final source alpha is exactly zero are discarded before fog/blend.
    /// This preserves non-zero-alpha edges and is intentionally limited to
    /// source-alpha sprite blend modes.
    pub fx_zero_alpha_discard: bool,
    /// Session-only A/B: force the uploaded BSP world vertex alpha to 1.
    pub debug_world_vertex_alpha_one: bool,
    /// Server-placed MD3 map props (misc_model_* / func_static models). Inline
    /// BSP brush models and geometry compiled into the BSP are unaffected.
    pub draw_map_models: bool,
    /// Draw inline BSP mover instances (func_rotating, doors, lifts).
    pub draw_inline_models: bool,
    /// r_fragmentShaderMode: 0 general world fragment program, 1 TaystJK-style
    /// per-material specialized programs (MaterialFragmentKey).
    pub fragment_shader_mode: u8,
    /// Session-only world submission ordering: 0 authored-sort baseline, 1 safe pipeline-first.
    pub world_sort_mode: u8,
    /// r_colorClearMode: 0 always clear, 1 skip when the sky box covers the background.
    pub color_clear_mode: u8,
    /// Render-only heuristic: keep BSP-authored stages whose shader qpath starts with models/.
    pub draw_bsp_model_surfaces: bool,
    /// Session diagnostic. 0 full, 1 opaque+masked only, 2 transparent only (no opaque depth).
    pub world_stage_probe: u8,
    /// TaystJK `r_fastsky`: 0 normal textured sky, 1 black, 2..9 palette, 10 red.
    pub fast_sky: u8,
    /// Debug overlays for trigger volumes and clip/collision-only brushes.
    /// Session-only: never written to the config.
    pub draw_triggers: bool,
    pub draw_clip_brushes: bool,
    /// `r_drawEntities`: NetRadiant-style colored boxes, classname labels and
    /// target/targetname link lines for every map entity. Session-only.
    pub draw_entities: bool,
    pub draw_fps: u8,
    /// Zero-based page of the complete live-thread CPU list.
    pub perf_thread_page: u32,
    /// `cg_drawTimer`: classic elapsed level timer (M:SS only).
    pub draw_timer: bool,
    /// `developer`: 0 quiet, 1 basic diagnostics, 2 verbose, 3 trace.
    /// `developer_tools` remains the derived compatibility gate used by
    /// inspector/overlay code: it is always `developer_level != 0`.
    pub developer_level: u8,
    pub developer_tools: bool,
    /// Classic renderer-only verbose spew level (`r_verbose`).
    pub renderer_verbose: u8,
    /// Session-only: log queued jobs and in-flight native BSP/RT PSOs.
    pub pipeline_trace: bool,
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
    /// Session-only command-encoding A/B controls; preserve baseline by default.
    pub model_state_cache: bool,
    pub dynamic_batch_draws: bool,
    /// Minimum visible members for CPU world compaction (1 baseline, 2 experimental).
    pub world_compact_min_draws: u32,
    pub model_prepare_threads: u32,
    pub md3_geometry_cache: bool,
    /// A/B GPU-resident instancing for repeated client-only static MD3 props.
    pub static_md3_instances: bool,
    /// Resident GPU instances for native vegetation surfaceSprites; false is
    /// the original per-frame CPU generation.
    pub surface_sprites_gpu: bool,
    /// r_surfaceSprites: draw the map's native JKA surfaceSprites at all.
    pub surface_sprites: bool,
    /// Use authored MD3 variants (always); optionally generate missing LODs in background.
    pub md3_auto_lod: bool,
    /// Developer A/B override for baked models. The shared r_md3AutoLod gate
    /// remains authoritative; this only disables the BSP subset.
    pub bsp_model_lod: bool,
    pub md3_lod_debug: bool,
    pub model_render_bundles: bool,
    pub encoder_finish_threads: u32,
    pub model_index_upload_cache: bool,
    pub ghoul2_batch_key_cache: bool,
    /// `r_ghoul2animsmooth`: jaPRO `CBoneCache::SmoothLow` renderer bone-history
    /// filter factor. Matches jaPRO's own default of 0.3; only active strictly
    /// between 0 and 1 (0 or >=1 disables it).
    pub ghoul2_anim_smooth: f32,
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
    /// 0 off, 1 female, 2 male, 3 both (reported player sex).
    pub jiggle_physics: u8,
    /// 0 = KawaiiPhysics-derived point solver, 1 = naelstrof/JigglePhysics Verlet solver.
    pub jiggle_solver: u8,
    pub jiggle_strength: f32,
    pub jiggle_breast_strength: f32,
    pub jiggle_glute_strength: f32,
    pub jiggle_stiffness: f32,
    pub jiggle_damping: f32,
    /// Deprecated legacy normalized remesh control kept for config compatibility.
    pub jiggle_glute_coverage_height: f32,
    /// Legacy normalized lower-trim cvar retained for config compatibility; the
    /// explicit world-unit Z controls below own remesh/motion height tuning.
    pub jiggle_glute_lift: f32,
    /// Signed model-space Z offsets, in JKA units, from the automatically fitted
    /// breast/glute remesh bounds. Bottom + raises the lower edge; top + raises
    /// the upper edge. These affect cached promoted topology only.
    pub jiggle_breast_remesh_bottom_z: f32,
    pub jiggle_breast_remesh_top_z: f32,
    pub jiggle_glute_remesh_bottom_z: f32,
    pub jiggle_glute_remesh_top_z: f32,
    /// Signed model-space Z offsets, in JKA units, from the default glute motion
    /// envelope. These affect deformation weighting only, never topology.
    pub jiggle_glute_jiggle_bottom_z: f32,
    pub jiggle_glute_jiggle_top_z: f32,
    /// Peak bind-pose mass added at the breast/glute region center, JKA units.
    /// Baked into the promoted jiggle mesh (rebuilds player models).
    pub jiggle_breast_size: f32,
    pub jiggle_glute_size: f32,
    pub body_muscle: f32,
    pub body_weight: f32,
    pub body_proportions: f32,
    pub body_age: f32,
    /// Origin-acceleration kick multiplier; steady travel never contributes.
    pub jiggle_locomotion: f32,
    /// Locomotion impulse ceiling per 60 Hz step; scaled for r_physicsHz.
    pub jiggle_impulse_cap: f32,
    /// Gyroscopic orbit coupling, 0 = straight bounce.
    pub jiggle_swirl: f32,
    /// Per-region KawaiiPhysics multipliers stacked on jiggle_stiffness/damping.
    pub jiggle_breast_stiffness: f32,
    pub jiggle_breast_damping: f32,
    pub jiggle_glute_stiffness: f32,
    pub jiggle_glute_damping: f32,
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
    /// HDR math is enabled for both Math Only and Full modes.
    pub hdr: bool,
    /// Internal swapchain request (0 SDR, 1 Auto, 2 PQ, 3 scRGB).
    /// Derived from r_hdr and r_hdrStyle, never configured independently.
    pub hdr_output: u32,
    /// 0 Auto, 1 HDR10/PQ, 2 scRGB (r_hdrStyle).
    pub hdr_style: u32,
    pub hdr_paper_white: f32,
    /// Zero uses the display's reported peak, falling back to 1000 nits.
    pub hdr_peak: f32,
    pub float_lightmap: bool,
    pub tone_mapping: f32,
    pub auto_exposure: f32,
    pub glow: GlowMode,
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
    /// 0 off, 1 regional temporal resolve, 2 influence-mask debug. Retains MSAA.
    pub rt_taa: u32,
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
    pub sun_shadow_strength: f32,
    pub sun_lighting: bool,
    /// Unnormalized RGB chosen by the user; renderer normalizes it like q3map.
    pub sun_color: [f32; 3],
    /// Controls whether direct shader-sun light requires an authored sky portal.
    pub sun_visibility: SunVisibilityMode,
    /// Strips the baked map sun out of the entity lightgrid sample and re-adds
    /// it as a directional light that follows the runtime sun (color, intensity,
    /// direction). Needs Entity ambient lighting = BSP lightgrid.
    pub entity_sun_lighting: bool,
    /// 0..1: estimate and replace baked map sunlight on BSP lightmapped surfaces.
    pub baked_sun_removal: f32,
    pub baked_sun_cache: bool,
    pub baked_sun_shadow_map: bool,
    pub baked_sun_edge_softness: f32,
    pub baked_sun_direct_limit: f32,
    pub baked_indirect_scale: f32,
    pub baked_indirect_saturation: f32,
    /// Nonlinear dimming of low irradiance: 1.0 = disabled.
    pub baked_indirect_power: f32,
    /// Keep high-luminance baked pools bright (heuristic; no light-source separation).
    pub baked_indirect_highlight_preserve: f32,
    /// 0 = map/default distanceCull; otherwise multiplier applied to that base.
    pub distance_cull_scale: f32,
    /// Experimental Star Nest procedural override of map-authored sky faces.
    pub sky_fractal_override: bool,
    pub sky_backend: u8,
    pub blue_water_environment: crate::blue_water_environment::Settings,
    pub cloud_backend: u8,
    /// Sky-portal geometry vs volumetric clouds: 0 off (clouds draw over portal
    /// geometry), 1 redraw portal depth, 2 reuse the portal pass depth.
    pub cloud_portal_depth: u8,
    pub clouds: bool,
    pub cloud_type: CloudType,
    pub cloud_quality: f32,
    pub cloud_coverage: f32,
    pub cloud_height: f32,
    pub cloud_thickness: f32,
    pub cloud_shadows: bool,
    /// One authoritative atmospheric wind shared by clouds, precipitation, grass, and ocean.
    pub weather_wind: crate::ocean::AtmosphericWind,
    /// Strength of camera-centred dust/motes and sparse gust wisps that make the shared atmospheric wind visible.
    pub wind_visuals: f32,
    /// Archived weather choice (`r_we`), independent of server world effects.
    pub weather_type: crate::world_effects::WeatherType,
    pub rain: bool,
    pub rain_intensity: RainIntensity,
    /// Rain on the camera's own lens: the RainyGlass droplet/rivulet refraction
    /// pass over the finished scene (`weather/rain_glass.rs`).
    pub rain_glass: bool,
    pub puddle_quality: PuddleQuality,
    /// How readily rain collects in scattered puddles on large flat ground, 0..1.
    pub puddle_scatter: f32,
    /// Strength of the wet-weather colour grade (cool shadows, warm highlights,
    /// richer neon) while it rains, 0..1.
    pub rain_grade: f32,
    /// Runtime A/B: omit rain's depth-aware haze/full spatial post requirement.
    /// Rain particles and wet world shading remain; rain haze/grade/final puddle post treatment is omitted.
    pub rain_fast_post: bool,
    /// Runtime A/B: use a prebaked map-space water-height field in the rain compute shader
    /// instead of scanning promoted-water volumes/outline triangles per drop.
    pub rain_water_field: bool,
    /// Runtime A/B: cheap radius math plus early center-point clip rejection in rain VS.
    pub rain_fast_vertex: bool,
    /// Runtime A/B: rain-only prepass writes only linear depth instead of motion/reflection MRTs.
    pub rain_lean_depth: bool,
    /// Runtime A/B: skip dynamic models in a rain-only depth prepass.
    pub rain_static_depth: bool,
    /// Runtime A/B: skip alpha-tested BSP in a rain-only depth prepass.
    pub rain_opaque_depth: bool,
    /// Runtime A/B: keep dynamic models in lean rain depth using R32-only entity pipelines.
    pub rain_lean_models: bool,
    /// Runtime A/B: use a rain-only specialized fullscreen post shader.
    pub rain_lean_post: bool,
    pub footprints: FootprintMode,
    pub grass: bool,
    /// Runtime A/B diagnostic: hoist root wind/clump work to a compute pass.
    pub grass_precompute: bool,
    /// Runtime A/B diagnostic: use the 5-triangle middle blade LOD.
    pub grass_mid_lod: bool,
    /// Runtime A/B diagnostic: order opaque grass near-to-far for early-Z.
    pub grass_front_to_back: bool,
    pub contact_shadow_debug: u8,
    /// `r_ssrDebug`: isolates the screen-space reflection pass. Runtime only.
    pub ssr_debug: u8,
    pub ocean: bool,
    /// cg_waterwake: 0 off, 1 TaystJK/JKA quad, 2 enhanced quad plus Godot interaction.
    pub water_wake: u8,
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
    pub dof_mode: DofMode,
    /// Native WGPU port of Timothy Lottes' crt-lottes-fast filter.
    pub crt_scope: CrtScope,
    pub crt_style: CrtStyle,
    pub crt_quality: CrtQuality,
    pub crt_reference: CrtReferenceResolution,
    pub crt_mask: CrtMask,
    pub crt_scanline: f32,
    pub crt_sharpness: f32,
    pub crt_mask_intensity: f32,
    pub crt_curvature: f32,
    pub crt_corner: f32,
    /// Crosshair-surface autofocus for DOF. When off, the last focus distance is held.
    pub dof_autofocus: bool,
    pub dof_quality: DofQuality,
    pub color_lut: ColorLutPreset,
    pub color_lut_strength: f32,
    pub color_grade: crate::color_grading::ColorGradeSettings,
    pub split_toning: crate::color_grading::SplitToningSettings,
    pub gpu_driven: bool,
    pub hiz_occlusion: bool,
    pub entity_ambient_lighting: EntityAmbientLightingMode,
    pub dynamic_lights: DynamicLightsMode,
    /// Diagnostic receiver view: neutral dark BSP surfaces plus runtime dynamic-light contribution only.
    pub dynamic_light_debug: bool,
    /// Infer transient lights from emissive legacy EFX that omit Light{} stages.
    pub promote_fx_lights: u8,
    /// Master for BSP shader-driven area-light promotion. q3map_surfacelight is
    /// intentionally separate and remains controlled by emissive_area_lights.
    pub promote_shader_lights: bool,
    pub promote_shader_glow: bool,
    pub promote_shader_additive: bool,
    pub promote_shader_emissive: bool,
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
    /// Standard OpenJK/TaystJK `r_flares`: authored RBSP MST_FLARE world flares.
    pub flares: bool,
    /// Experiment: permit conservative reverse-lightmap pair fusion when the
    /// CPU world-material recipe is rebuilt from its retained baseline.
    pub stage_collapse_extra: bool,
    /// Reversible CPU-prepared composite of compatible consecutive additive world stages.
    /// Disable for exact authored multi-draw A/B comparisons (cold GPU world reupload).
    pub additive_stage_fusion: bool,
    /// Optional world-flare override; either world cvar enables the same renderer.
    pub world_flares: bool,
    /// Screen-space saber clash flare overlay, separate from authored hit EFX.
    pub saber_flares: bool,
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
    /// Experimental RGB glass throughput for hardware RT local lights. Default OFF.
    pub tinted_glass: bool,
    /// 0=off, 1=affected receiver overlay, 2=transmitted/blocked RGB debug.
    pub tinted_glass_debug: u32,
    pub cascaded_shadows: bool,
    pub cull_debug: CullDebugMode,
    pub planar_reflection_debug: PlanarReflectionDebugMode,
}

impl VideoSettings {
    pub(crate) fn body_shape_params(&self) -> jka_assets::body_shape::BodyShapeParams {
        jka_assets::body_shape::BodyShapeParams {
            muscle: self.body_muscle,
            weight: self.body_weight,
            proportions: self.body_proportions,
            age: self.body_age,
        }
    }

    /// r_jigglePhysics: 0 off, 1 female, 2 male, 3 both. Textual on/off
    /// remain accepted for older config scripts.
    pub(crate) fn parse_jiggle_mode(value: &str) -> Option<u8> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" => Some(0),
            "1" | "female" | "f" => Some(1),
            "2" | "male" | "m" => Some(2),
            "3" | "both" | "on" | "true" => Some(3),
            _ => None,
        }
    }

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

impl VideoSettings {
    /// Both names enable the same RBSP renderer: `r_worldFlares` is an
    /// independent override, not a second rendering pass. No duplicate draws.
    pub fn world_flares_active(&self) -> bool {
        self.flares || self.world_flares
    }
}

#[cfg(test)]
mod flare_cvar_tests {
    use super::VideoSettings;

    #[test]
    fn world_flares_are_enabled_by_either_cvar_without_affecting_saber_flares() {
        let mut settings = VideoSettings::default();
        assert!(settings.world_flares_active());
        assert!(settings.saber_flares);

        settings.flares = false;
        settings.world_flares = false;
        assert!(!settings.world_flares_active());

        settings.world_flares = true;
        assert!(settings.world_flares_active());
        settings.saber_flares = false;
        assert!(settings.world_flares_active());

        settings.world_flares = false;
        settings.flares = true;
        assert!(settings.world_flares_active());
        assert!(!settings.saber_flares);
    }
}

impl Default for VideoSettings {
    fn default() -> Self {
        let mut settings = Self {
            patch_subdivisions: 4.0,
            patch_stitching: true,
            lod_curve_error: 250.0,
            no_curves: false,
            speeds: 0,
            fullscreen: FullscreenMode::Windowed,
            renderer_backend: RendererBackend::Vulkan,
            gpu_adapter: 0,
            vsync: VsyncMode::Fast,
            max_frame_latency: 3,
            early_submit: false,
            present_copy: false,
            texture_compression: 0,
            msaa_samples: 1,
            resolution: [1280, 800],
            window_position: None,
            window_maximized: false,
            texture_filter: TextureFilter::Trilinear,
            picmip: 0,
            detail_textures: false,
            detail_texture_fade: false,
            detail_texture_fade_distance: 512.0,
            wireframe_mask: 0,
            shader_complexity: false,
            skip_ui: false,
            draw_2d: true,
            draw_hud: true,
            pvs_mode: PvsMode::Auto,
            face_plane_cull: false,
            fps_cap: 0,
            fx_fps: crate::fx::FX_FPS_DEFAULT,
            fx_fps_scope: crate::fx::FX_FPS_SCOPE_DEFAULT,
            fx_physics: crate::fx::FX_PHYSICS_DEFAULT,
            fx_lod: crate::fx::FX_LOD_DEFAULT,
            fx_count_scale: 1.0,
            fx_lod_scale: crate::fx::LOD_SCALE_DEFAULT,
            lod_scale: crate::fx::LOD_SCALE_DEFAULT,
            fx_geometry: FxGeometryMode::Gpu,
            fx_particle_simulation: true,
            fx_zero_alpha_discard: false,
            debug_world_vertex_alpha_one: false,
            draw_map_models: true,
            draw_inline_models: true,
            fragment_shader_mode: 1,
            world_sort_mode: 0,
            color_clear_mode: 1,
            draw_bsp_model_surfaces: true,
            world_stage_probe: 0,
            fast_sky: 0,
            draw_triggers: false,
            draw_clip_brushes: false,
            draw_entities: false,
            draw_fps: Default::default(),
            perf_thread_page: 0,
            draw_timer: Default::default(),
            developer_level: 0,
            developer_tools: false,
            renderer_verbose: 0,
            pipeline_trace: false,
            perf_trace: false,
            force_unified_world: false,
            pom: true,
            gpu_timings: false,
            ghoul2_skinning: Ghoul2SkinningMode::Gpu,
            ghoul2_early_cull: true,
            ghoul2_lod_bias: 0,
            ghoul2_batch_draws: Ghoul2BatchMode::Adaptive,
            model_state_cache: false,
            dynamic_batch_draws: true,
            world_compact_min_draws: 1,
            model_prepare_threads: 0,
            md3_geometry_cache: true,
            static_md3_instances: false,
            surface_sprites_gpu: true,
            surface_sprites: true,
            md3_auto_lod: true,
            bsp_model_lod: true,
            md3_lod_debug: false,
            model_render_bundles: false,
            encoder_finish_threads: 2,
            model_index_upload_cache: true,
            ghoul2_batch_key_cache: true,
            ghoul2_anim_smooth: 0.3,
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
            jiggle_physics: 0,
            jiggle_solver: 0,
            jiggle_strength: 1.0,
            jiggle_breast_strength: 1.0,
            jiggle_glute_strength: 1.0,
            jiggle_stiffness: 1.0,
            jiggle_damping: 1.0,
            jiggle_glute_coverage_height: 1.0,
            jiggle_glute_lift: 0.0,
            jiggle_breast_remesh_bottom_z: 0.0,
            jiggle_breast_remesh_top_z: 0.0,
            jiggle_glute_remesh_bottom_z: 0.0,
            jiggle_glute_remesh_top_z: 0.0,
            jiggle_glute_jiggle_bottom_z: 0.0,
            jiggle_glute_jiggle_top_z: 0.0,
            jiggle_breast_size: 0.0,
            jiggle_glute_size: 0.0,
            body_muscle: 0.5,
            body_weight: 0.5,
            body_proportions: 0.5,
            body_age: 0.5,
            jiggle_locomotion: 1.0,
            jiggle_impulse_cap: 0.5,
            jiggle_swirl: 0.35,
            jiggle_breast_stiffness: 1.0,
            jiggle_breast_damping: 1.0,
            jiggle_glute_stiffness: 1.0,
            jiggle_glute_damping: 1.0,
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
            hdr_output: 0,
            hdr_style: 0,
            hdr_paper_white: 200.0,
            hdr_peak: 0.0,
            float_lightmap: false,
            tone_mapping: 0.0,
            auto_exposure: 0.0,
            glow: GlowMode::Off,
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
            rt_taa: 0,
            contact_shadows: false,
            fog_mode: FogMode::Off,
            fog_strength: 0.0,
            sun_override: false,
            // Matches the renderer's legacy fallback direction when a map has no authored sun.
            sun_yaw: 314.25595,
            sun_pitch: 57.04725,
            sun_intensity: 250.0,
            sun_shadow_strength: 0.3,
            sun_lighting: false,
            sun_color: [1.0, 1.0, 1.0],
            sun_visibility: SunVisibilityMode::SkyPortals,
            entity_sun_lighting: false,
            baked_sun_removal: 0.0,
            baked_sun_cache: true,
            baked_sun_shadow_map: true,
            baked_sun_edge_softness: 2.5,
            baked_sun_direct_limit: 0.48,
            baked_indirect_scale: 1.0,
            baked_indirect_saturation: 1.0,
            baked_indirect_power: 1.0,
            baked_indirect_highlight_preserve: 1.0,
            distance_cull_scale: 0.0,
            sky_fractal_override: false,
            sky_backend: 0,
            blue_water_environment: crate::blue_water_environment::Settings::default(),
            cloud_backend: 0,
            cloud_portal_depth: 0,
            clouds: true,
            cloud_type: CloudType::Storm,
            cloud_quality: 1.0,
            cloud_coverage: 0.6,
            cloud_height: 4600.0,
            cloud_thickness: 1200.0,
            cloud_shadows: true,
            weather_wind: crate::ocean::AtmosphericWind {
                speed: 167.0,
                direction: 220.0,
                gust: 0.2,
                shift: 0.0,
            },
            wind_visuals: 0.0,
            weather_type: crate::world_effects::WeatherType::Clear,
            rain: false,
            rain_intensity: RainIntensity::Rain,
            rain_glass: true,
            puddle_quality: PuddleQuality::High,
            puddle_scatter: 0.8,
            rain_grade: 0.5,
            rain_fast_post: false,
            rain_water_field: false,
            rain_fast_vertex: false,
            rain_lean_depth: false,
            rain_static_depth: false,
            rain_opaque_depth: false,
            rain_lean_models: false,
            rain_lean_post: false,
            footprints: FootprintMode::ThreeD,
            grass: true,
            grass_precompute: true,
            grass_mid_lod: true,
            grass_front_to_back: true,
            contact_shadow_debug: 0,
            ssr_debug: 0,
            ocean: false,
            water_wake: 1,
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
            dof_mode: DofMode::Off,
            crt_scope: CrtScope::Off,
            crt_style: CrtStyle::LottesFast,
            crt_quality: CrtQuality::Fast,
            crt_reference: CrtReferenceResolution::P480,
            crt_mask: CrtMask::GrilleLite,
            crt_scanline: 0.5,
            crt_sharpness: 2.5,
            crt_mask_intensity: 0.5,
            crt_curvature: 0.02,
            crt_corner: 3.0,
            dof_autofocus: true,
            dof_quality: DofQuality::Adaptive,
            color_lut: ColorLutPreset::Off,
            color_lut_strength: 1.0,
            color_grade: Default::default(),
            split_toning: Default::default(),
            gpu_driven: false,
            hiz_occlusion: false,
            entity_ambient_lighting: EntityAmbientLightingMode::Off,
            dynamic_lights: DynamicLightsMode::Off,
            dynamic_light_debug: false,
            promote_fx_lights: 0,
            promote_shader_lights: false,
            promote_shader_glow: true,
            promote_shader_additive: true,
            promote_shader_emissive: true,
            rt_samples: 1,
            dynamic_light_falloff: 0,
            rt_half_resolution: false,
            map_light_simulation: false,
            world_lighting: true,
            vertex_lighting: false,
            lightmap_only: false,
            modern_sabers: false,
            flares: true,
            stage_collapse_extra: false,
            additive_stage_fusion: true,
            world_flares: false,
            saber_flares: true,
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
            tinted_glass: false,
            tinted_glass_debug: 0,
            cascaded_shadows: false,
            cull_debug: CullDebugMode::Off,
            planar_reflection_debug: PlanarReflectionDebugMode::Off,
        };
        crate::app::video_cvar_registry().apply_defaults(&mut settings)
            .expect("registered video defaults must apply");
        settings
    }
}

// One external HDR setting; the underlying boolean and swapchain request are
// only implementation details. Do not add separately archived cvars for them.
impl VideoSettings {
    pub fn hdr_mode(&self) -> u32 {
        if self.hdr_output != 0 { 2 } else { u32::from(self.hdr) }
    }

    pub fn set_hdr_mode(&mut self, mode: u32) {
        self.hdr = mode != 0;
        self.hdr_output = if mode == 2 { self.hdr_style.min(2) + 1 } else { 0 };
    }

    pub fn set_hdr_style(&mut self, style: u32) {
        self.hdr_style = style.min(2);
        if self.hdr_output != 0 {
            self.hdr_output = self.hdr_style + 1;
        }
    }
}

#[cfg(test)]
mod hdr_mode_tests {
    use super::VideoSettings;
    #[test]
    fn output_is_only_requested_by_full_mode() {
        let mut settings = VideoSettings::default();
        settings.set_hdr_style(2);
        assert_eq!(settings.hdr_output, 0);
        settings.set_hdr_mode(1);
        assert!(settings.hdr);
        assert_eq!(settings.hdr_output, 0);
        settings.set_hdr_mode(2);
        assert_eq!(settings.hdr_output, 3);
        settings.set_hdr_mode(0);
        assert!(!settings.hdr);
        assert_eq!(settings.hdr_output, 0);
    }
}
