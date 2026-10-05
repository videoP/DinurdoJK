//! Reset.
use crate::app::egui_settings::{
    theme, ui, App, DynamicShadowsMode, QualityPreset, RenderCommand, VideoSettings,
    OCEAN_FOAM_COLOR, OCEAN_MAP_SIZE, OCEAN_MESH_QUALITY, OCEAN_NORMAL_STRENGTH, OCEAN_ROUGHNESS,
    OCEAN_SEA_SPRAY, OCEAN_UPDATES, OCEAN_WATER_COLOR, OCEAN_WIND_FOAM, PHYS_CCD,
    PHYS_CLIENT_ENABLED, PHYS_CLOTH, PHYS_CLOTH_AIR, PHYS_CLOTH_ANIMATION,
    PHYS_CLOTH_BODY_COLLISION, PHYS_CLOTH_CLEARANCE, PHYS_CLOTH_TURN, PHYS_CLOTH_WIND, PHYS_DEBRIS,
    PHYS_DEBRIS_LIFETIME, PHYS_DEBRIS_MAX, PHYS_DEBUG_DRAW, PHYS_DISMEMBERMENT,
    PHYS_DISMEMBER_LIFETIME, PHYS_DISMEMBER_MAX, PHYS_EXPLOSION_IMPULSES, PHYS_FORCE_IMPULSES,
    PHYS_JIGGLE, PHYS_JIGGLE_BREAST, PHYS_JIGGLE_DAMPING, PHYS_JIGGLE_GLUTE,
    PHYS_JIGGLE_GLUTE_LIFT, PHYS_JIGGLE_JP_AIR_DRAG, PHYS_JIGGLE_JP_DRAG,
    PHYS_JIGGLE_JP_GRAVITY, PHYS_JIGGLE_JP_SOFTEN, PHYS_JIGGLE_JP_STIFFNESS,
    PHYS_JIGGLE_JP_STRETCH, PHYS_JIGGLE_SOLVER, PHYS_JIGGLE_STIFFNESS, PHYS_JIGGLE_STRENGTH,
    PHYS_MAX_SUBSTEPS,
    PHYS_PLAYER_PUSH, PHYS_PROPS, PHYS_PROP_MAX, PHYS_RAGDOLLS, PHYS_RAGDOLL_LIFETIME,
    PHYS_RAGDOLL_MAX, PHYS_RAGDOLL_SELF_COLLISION, PHYS_RATE, PHYS_SLEEPING, PHYS_STATS,
    PHYS_WEAPON_IMPULSES,
};

// ---------------------------------------------------------------- resets --

impl App {
    /// Restore one setting to the value a fresh install would have.
    ///
    /// Each arm mirrors the corresponding arm of `change_video_setting` /
    /// `change_environment_setting`: same field, same renderer sync, same
    /// dirty-marking. Assigning the default directly rather than cycling the
    /// control to it avoids walking through every intermediate state, which for
    /// anti-aliasing or the backend would mean several pipeline rebuilds on one
    /// click.
    pub(in crate::app) fn apply_setting_reset(&mut self, reset: theme::Reset) {
        match reset {
            theme::Reset::None => {}
            theme::Reset::Color(name) => {
                let defaults = crate::color_grading::SplitToningSettings::default();
                let value = match name {
                    "r_gammaMethod" => "shader".to_owned(),
                    "r_splitToning" => u8::from(defaults.enabled).to_string(),
                    "r_splitToningStrength" => defaults.strength.to_string(),
                    "r_splitToningShadowHue" => defaults.shadow_hue.to_string(),
                    "r_splitToningShadowSaturation" => defaults.shadow_saturation.to_string(),
                    "r_splitToningHighlightHue" => defaults.highlight_hue.to_string(),
                    "r_splitToningHighlightSaturation" => defaults.highlight_saturation.to_string(),
                    "r_splitToningBalance" => defaults.balance.to_string(),
                    _ => return,
                };
                let _ = self.set_console_cvar(name, &value);
            }

            theme::Reset::ColorPair(hue_name, saturation_name) => {
                let defaults = crate::color_grading::SplitToningSettings::default();
                let (hue, saturation) = match (hue_name, saturation_name) {
                    ("r_splitToningShadowHue", "r_splitToningShadowSaturation") => {
                        (defaults.shadow_hue, defaults.shadow_saturation)
                    }
                    ("r_splitToningHighlightHue", "r_splitToningHighlightSaturation") => {
                        (defaults.highlight_hue, defaults.highlight_saturation)
                    }
                    _ => return,
                };
                let _ = self.set_console_cvar(hue_name, &hue.to_string());
                let _ = self.set_console_cvar(saturation_name, &saturation.to_string());
            }

            theme::Reset::Video(row) => self.reset_video_row(row),
            theme::Reset::Environment(row) => self.reset_environment_row(row),
            theme::Reset::CloudTuning(row) => self.reset_cloud_tuning_row(row),
            theme::Reset::Physics(field) => self.reset_physics_field(field),
            theme::Reset::Ocean(field) => self.reset_ocean_global(field),
            theme::Reset::OceanCascade(cascade, field) => {
                self.reset_ocean_cascade(cascade as usize, field)
            }
        }
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    pub(in crate::app::egui_settings) fn reset_video_row(&mut self, row: usize) {
        let defaults = VideoSettings::default();
        match row {
            ui::VIDEO_ROW_FULLSCREEN => self.set_fullscreen_mode(defaults.fullscreen),
            ui::VIDEO_ROW_RESOLUTION => {
                self.video.resolution = defaults.resolution;
                self.mark_config_dirty();
                self.console_status = format!(
                    "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                    self.video.resolution[0], self.video.resolution[1]
                );
            }
            ui::VIDEO_ROW_VSYNC => {
                self.video.vsync = defaults.vsync;
                self.render_command(RenderCommand::SetVsync(self.video.vsync));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_MAX_FRAME_LATENCY => {
                self.set_max_frame_latency(defaults.max_frame_latency);
            }
            ui::VIDEO_ROW_QUALITY_PRESET => self.apply_quality_preset(QualityPreset::Low),
            ui::VIDEO_ROW_FPS_CAP => self.set_fps_cap(defaults.fps_cap),
            ui::VIDEO_ROW_PHYSICS_FPS => self.set_physics_msec(defaults.physics_msec),
            ui::VIDEO_ROW_BRIGHTNESS => self.set_gamma(defaults.gamma),
            ui::VIDEO_ROW_MODEL_BRIGHTNESS => {
                self.set_model_brightness_locked(defaults.model_brightness_locked)
            }
            ui::VIDEO_ROW_DLIGHT_BRIGHTNESS => {
                self.set_dynamic_light_brightness_locked(defaults.dynamic_light_brightness_locked)
            }
            ui::VIDEO_ROW_DRAW_TRIGGERS => {
                self.video.draw_triggers = defaults.draw_triggers;
                self.sync_debug_volumes();
            }
            ui::VIDEO_ROW_DRAW_CLIP_BRUSHES => {
                self.video.draw_clip_brushes = defaults.draw_clip_brushes;
                self.sync_debug_volumes();
            }
            ui::VIDEO_ROW_DRAW_ENTITIES => {
                self.set_draw_entities(defaults.draw_entities);
            }
            ui::VIDEO_ROW_DRAW_MAP_MODELS => {
                self.video.draw_map_models = defaults.draw_map_models;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_ANTI_ALIASING => {
                self.video.msaa_samples = defaults.msaa_samples;
                self.video.fxaa = defaults.fxaa;
                self.video.smaa = defaults.smaa;
                self.video.taa = defaults.taa;
                self.render_command(RenderCommand::SetMsaa(self.video.msaa_samples));
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_TEXTURE_FILTER => {
                self.video.texture_filter = defaults.texture_filter;
                self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PICMIP => {
                self.video.picmip = defaults.picmip;
                self.render_command(RenderCommand::SetPicmip(self.video.picmip));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DETAIL_TEXTURES => {
                self.video.detail_textures = defaults.detail_textures;
                self.video.detail_texture_fade = defaults.detail_texture_fade;
                self.video.detail_texture_fade_distance = defaults.detail_texture_fade_distance;
                self.render_command(RenderCommand::SetDetailTextures(self.video.detail_textures));
                self.render_command(RenderCommand::SetDetailTextureFade {
                    enabled: self.video.detail_texture_fade,
                    distance: self.video.detail_texture_fade_distance,
                });
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PVS => {
                self.video.pvs_mode = defaults.pvs_mode;
                self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DISTANCE_CULL => {
                self.set_distance_cull_scale(defaults.distance_cull_scale)
            }
            ui::VIDEO_ROW_GPU_DRIVEN => {
                self.video.gpu_driven = defaults.gpu_driven;
                self.sync_gpu_visibility();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_HIZ => {
                self.video.hiz_occlusion = defaults.hiz_occlusion;
                self.sync_gpu_visibility();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING => {
                self.video.entity_ambient_lighting = defaults.entity_ambient_lighting;
                self.sync_entity_ambient_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PBR => {
                self.video.pbr = defaults.pbr;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_ASSET_OVERRIDES => {
                self.video.allow_asset_overrides = defaults.allow_asset_overrides;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GEN_NORMAL_MAPS => {
                self.video.gen_normal_maps = defaults.gen_normal_maps;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DELUXE_MAPPING => {
                self.video.deluxe_mapping = defaults.deluxe_mapping;
                self.sync_pbr();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DELUXE_SPECULAR => {
                self.video.deluxe_specular = defaults.deluxe_specular;
                self.sync_pbr();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_WORLD_LIGHTING => {
                self.video.world_lighting = defaults.world_lighting;
                self.video.vertex_lighting = defaults.vertex_lighting;
                self.sync_classic_world_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_VERTEX_LIGHTING => {
                self.video.vertex_lighting = defaults.vertex_lighting;
                self.sync_classic_world_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_LIGHTMAP_ONLY => {
                self.video.lightmap_only = defaults.lightmap_only;
                self.sync_classic_world_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DYNAMIC_LIGHTS => {
                self.video.dynamic_lights = defaults.dynamic_lights;
                self.sync_dynamic_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_MAP_LIGHT_SIMULATION => {
                self.video.map_light_simulation = defaults.map_light_simulation;
                self.sync_map_light_simulation();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FX_GEOMETRY => {
                self.video.fx_geometry = defaults.fx_geometry;
                self.mark_config_dirty();
                self.console_status = format!("FX GEOMETRY: {}", self.video.fx_geometry.label());
            }
            ui::VIDEO_ROW_FX_ZERO_ALPHA_DISCARD => {
                self.video.fx_zero_alpha_discard = defaults.fx_zero_alpha_discard;
                self.render_command(crate::renderer::RenderCommand::SetFxZeroAlphaDiscard(
                    self.video.fx_zero_alpha_discard,
                ));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FX_FPS => {
                self.video.fx_fps = defaults.fx_fps;
                self.apply_fx_fps_settings();
                self.mark_config_dirty();
                self.console_status = format!("FX FPS: {} HZ", self.video.fx_fps);
            }
            ui::VIDEO_ROW_FX_FPS_SCOPE => {
                self.video.fx_fps_scope = defaults.fx_fps_scope;
                self.apply_fx_fps_settings();
                self.mark_config_dirty();
                self.console_status = "FX FPS SCOPE: CONTINUOUS EFX".into();
            }
            ui::VIDEO_ROW_FX_LOD => {
                self.video.fx_lod = defaults.fx_lod;
                self.apply_fx_lod();
                self.mark_config_dirty();
                self.console_status =
                    format!("FX LOD: {}", crate::fx::lod_label(self.video.fx_lod));
            }
            ui::VIDEO_ROW_FX_PHYSICS => {
                self.video.fx_physics = defaults.fx_physics;
                self.apply_fx_physics();
                self.mark_config_dirty();
                self.console_status = format!(
                    "FX PHYSICS: {}",
                    crate::fx::physics_label(self.video.fx_physics)
                );
            }
            ui::VIDEO_ROW_MODERN_SABERS => {
                self.video.modern_sabers = defaults.modern_sabers;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FLARES => {
                self.video.flares = defaults.flares;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_SABER_IMPACT_FX => {
                self.video.saber_impact_fx = defaults.saber_impact_fx;
                if let Some(session) = self.game_session.as_mut() {
                    session
                        .weapon_fx
                        .set_saber_impact_fx(self.video.saber_impact_fx);
                }
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_SABER_MARKS => {
                self.video.saber_marks = defaults.saber_marks;
                if let Some(session) = self.game_session.as_mut() {
                    session.weapon_fx.set_saber_marks(self.video.saber_marks);
                }
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS => {
                self.video.emissive_area_lights = defaults.emissive_area_lights;
                self.sync_emissive_area_lights();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_AMBIENT_OCCLUSION => {
                self.video.ssao = defaults.ssao;
                self.video.static_bsp_ao = defaults.static_bsp_ao;
                self.video.static_bsp_ao_lightmap = defaults.static_bsp_ao_lightmap;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_VOXEL_PROBE_GI => {
                self.video.voxel_probe_gi = defaults.voxel_probe_gi;
                self.sync_voxel_probe_gi();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DYNAMIC_SHADOWS => {
                self.video.dynamic_shadows = defaults.dynamic_shadows;
                self.video.cascaded_shadows =
                    self.video.dynamic_shadows == DynamicShadowsMode::CascadedShadowMaps;
                self.sync_cascaded_shadows();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_ENTITY_SHADOW_LIGHT => {
                self.video.entity_shadow_light = defaults.entity_shadow_light;
                self.sync_entity_shadow_light();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS => {
                self.video.local_light_shadows = defaults.local_light_shadows;
                self.sync_local_light_shadows();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CONTACT_SHADOWS => {
                self.video.contact_shadows = defaults.contact_shadows;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_SSR => {
                self.video.reflection_quality = defaults.reflection_quality;
                self.reflection_quality_changed();
            }
            ui::VIDEO_ROW_PLANAR_REFLECTIONS => {
                self.video.reflection_debug = defaults.reflection_debug;
                self.sync_post_effects();
            }
            ui::VIDEO_ROW_PLANAR_REFLECTION_DEBUG => {
                self.video.planar_reflection_debug = defaults.planar_reflection_debug;
                self.sync_planar_reflection_debug();
            }
            ui::VIDEO_ROW_HDR => {
                self.video.hdr = defaults.hdr;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FLOAT_LIGHTMAP => {
                self.video.float_lightmap = defaults.float_lightmap;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_TONE_MAPPING => {
                self.video.tone_mapping = defaults.tone_mapping;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_AUTO_EXPOSURE => {
                self.video.auto_exposure = defaults.auto_exposure;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BLOOM => {
                self.video.bloom = defaults.bloom;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_MOTION_BLUR => {
                self.set_motion_blur_strength(defaults.motion_blur_strength)
            }
            ui::VIDEO_ROW_DEPTH_OF_FIELD => {
                self.set_depth_of_field_strength(defaults.depth_of_field_strength)
            }
            ui::VIDEO_ROW_DOF_AUTOFOCUS => {
                self.video.dof_autofocus = defaults.dof_autofocus;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DOF_QUALITY => {
                self.video.dof_quality = defaults.dof_quality;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_HALATION => {
                self.video.halation = defaults.halation;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CHROMATIC_ABERRATION => {
                self.set_chromatic_aberration_strength(defaults.chromatic_aberration)
            }
            ui::VIDEO_ROW_VIGNETTE => {
                self.video.vignette = defaults.vignette;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FILM_GRAIN => self.set_film_grain_strength(defaults.film_grain_strength),
            ui::VIDEO_ROW_COLOR_LUT => self.set_color_lut(defaults.color_lut),
            ui::VIDEO_ROW_LUT_STRENGTH => self.set_color_lut_strength(defaults.color_lut_strength),
            ui::VIDEO_ROW_WIREFRAME => {
                self.video.wireframe_mask = defaults.wireframe_mask;
                self.render_command(RenderCommand::SetWireframeMask(self.video.wireframe_mask));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CULL_DEBUG => {
                self.video.cull_debug = defaults.cull_debug;
                self.sync_cull_debug();
            }
            ui::VIDEO_ROW_DRAW_FPS => {
                self.video.draw_fps = defaults.draw_fps;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DEVELOPER_TOOLS => {
                self.video.developer_level = defaults.developer_level;
                self.video.developer_tools = self.video.developer_level != 0;
                crate::logging::set_developer_level(self.video.developer_level);
                if !self.video.developer_tools {
                    self.surface_inspector = None;
                    self.render_command(RenderCommand::ClearSurfaceInspection);
                }
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_RENDERER_VERBOSE => {
                self.video.renderer_verbose = defaults.renderer_verbose;
                crate::logging::set_renderer_verbose_level(self.video.renderer_verbose);
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PERF_TRACE => {
                self.video.perf_trace = defaults.perf_trace;
                self.render_command(RenderCommand::SetPerfTrace(self.video.perf_trace));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GPU_TIMINGS => {
                self.video.gpu_timings = defaults.gpu_timings;
                self.render_command(RenderCommand::SetGpuTimings(self.video.gpu_timings));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_SKINNING => {
                self.video.ghoul2_skinning = defaults.ghoul2_skinning;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_LOD_BIAS => {
                self.video.ghoul2_lod_bias = defaults.ghoul2_lod_bias;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_BATCH_DRAWS => {
                self.video.ghoul2_batch_draws = defaults.ghoul2_batch_draws;
                self.render_command(RenderCommand::SetGhoul2BatchDraws(
                    self.video.ghoul2_batch_draws,
                ));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_EARLY_CULL => {
                self.video.ghoul2_early_cull = defaults.ghoul2_early_cull;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_RENDER_BACKEND => {
                self.video.renderer_backend = defaults.renderer_backend;
                self.mark_config_dirty();
                self.console_status = format!(
                    "RENDER BACKEND: {} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                    self.video.renderer_backend.label()
                );
            }
            ui::VIDEO_ROW_BAKED_AO_SAMPLES => {
                self.video.static_bsp_ao_samples = defaults.static_bsp_ao_samples;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_RESOLUTION => {
                self.video.static_bsp_ao_resolution = defaults.static_bsp_ao_resolution;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_STRENGTH => {
                self.video.static_bsp_ao_strength = defaults.static_bsp_ao_strength;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_RANGE => {
                self.video.static_bsp_ao_range = defaults.static_bsp_ao_range;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_CURRENT_CELL => {
                self.video.static_bsp_ao_current_cell = defaults.static_bsp_ao_current_cell;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            _ => {}
        }
    }

    pub(in crate::app::egui_settings) fn reset_physics_field(&mut self, field: u8) {
        let defaults = VideoSettings::default();
        match field {
            PHYS_CLIENT_ENABLED => self.video.client_physics = defaults.client_physics,
            PHYS_RATE => self.video.client_physics_hz = defaults.client_physics_hz,
            PHYS_MAX_SUBSTEPS => {
                self.video.client_physics_max_substeps = defaults.client_physics_max_substeps
            }
            PHYS_CCD => self.video.client_physics_ccd = defaults.client_physics_ccd,
            PHYS_SLEEPING => self.video.client_physics_sleeping = defaults.client_physics_sleeping,
            PHYS_RAGDOLLS => self.video.ragdolls = defaults.ragdolls,
            PHYS_RAGDOLL_MAX => self.video.ragdoll_max = defaults.ragdoll_max,
            PHYS_RAGDOLL_LIFETIME => self.video.ragdoll_lifetime = defaults.ragdoll_lifetime,
            PHYS_RAGDOLL_SELF_COLLISION => {
                self.video.ragdoll_self_collision = defaults.ragdoll_self_collision
            }
            PHYS_JIGGLE => self.video.jiggle_physics = defaults.jiggle_physics,
            PHYS_JIGGLE_STRENGTH => self.video.jiggle_strength = defaults.jiggle_strength,
            PHYS_JIGGLE_BREAST => {
                self.video.jiggle_breast_strength = defaults.jiggle_breast_strength
            }
            PHYS_JIGGLE_GLUTE => self.video.jiggle_glute_strength = defaults.jiggle_glute_strength,
            PHYS_JIGGLE_STIFFNESS => self.video.jiggle_stiffness = defaults.jiggle_stiffness,
            PHYS_JIGGLE_DAMPING => self.video.jiggle_damping = defaults.jiggle_damping,
            PHYS_JIGGLE_GLUTE_LIFT => self.video.jiggle_glute_lift = defaults.jiggle_glute_lift,
            PHYS_JIGGLE_SOLVER => self.video.jiggle_solver = defaults.jiggle_solver,
            PHYS_JIGGLE_JP_STIFFNESS => self.video.jiggle_jp_stiffness = defaults.jiggle_jp_stiffness,
            PHYS_JIGGLE_JP_DRAG => self.video.jiggle_jp_drag = defaults.jiggle_jp_drag,
            PHYS_JIGGLE_JP_AIR_DRAG => self.video.jiggle_jp_air_drag = defaults.jiggle_jp_air_drag,
            PHYS_JIGGLE_JP_STRETCH => self.video.jiggle_jp_stretch = defaults.jiggle_jp_stretch,
            PHYS_JIGGLE_JP_SOFTEN => self.video.jiggle_jp_soften = defaults.jiggle_jp_soften,
            PHYS_JIGGLE_JP_GRAVITY => self.video.jiggle_jp_gravity = defaults.jiggle_jp_gravity,
            PHYS_DISMEMBERMENT => self.video.dismemberment = defaults.dismemberment,
            PHYS_DISMEMBER_MAX => self.video.dismember_max = defaults.dismember_max,
            PHYS_DISMEMBER_LIFETIME => self.video.dismember_lifetime = defaults.dismember_lifetime,
            PHYS_CLOTH => self.video.cloth_physics = defaults.cloth_physics,
            PHYS_CLOTH_BODY_COLLISION => {
                self.video.cloth_body_collision = defaults.cloth_body_collision
            }
            PHYS_CLOTH_WIND => self.video.cloth_wind = defaults.cloth_wind,
            PHYS_CLOTH_AIR => self.video.cloth_air_resistance = defaults.cloth_air_resistance,
            PHYS_CLOTH_TURN => self.video.cloth_turn_response = defaults.cloth_turn_response,
            PHYS_CLOTH_ANIMATION => {
                self.video.cloth_animation_influence = defaults.cloth_animation_influence
            }
            PHYS_CLOTH_CLEARANCE => self.video.cloth_body_clearance = defaults.cloth_body_clearance,
            PHYS_PROPS => self.video.physics_props = defaults.physics_props,
            PHYS_PROP_MAX => self.video.physics_prop_max = defaults.physics_prop_max,
            PHYS_DEBRIS => self.video.physics_debris = defaults.physics_debris,
            PHYS_DEBRIS_MAX => self.video.physics_debris_max = defaults.physics_debris_max,
            PHYS_DEBRIS_LIFETIME => {
                self.video.physics_debris_lifetime = defaults.physics_debris_lifetime
            }
            PHYS_PLAYER_PUSH => self.video.physics_player_push = defaults.physics_player_push,
            PHYS_WEAPON_IMPULSES => {
                self.video.physics_weapon_impulses = defaults.physics_weapon_impulses
            }
            PHYS_EXPLOSION_IMPULSES => {
                self.video.physics_explosion_impulses = defaults.physics_explosion_impulses
            }
            PHYS_FORCE_IMPULSES => {
                self.video.physics_force_impulses = defaults.physics_force_impulses
            }
            PHYS_DEBUG_DRAW => self.video.physics_debug_draw = defaults.physics_debug_draw,
            PHYS_STATS => self.video.physics_stats = defaults.physics_stats,
            _ => return,
        }
        self.mark_config_dirty();
    }

    pub(in crate::app::egui_settings) fn reset_environment_row(&mut self, row: usize) {
        let defaults = VideoSettings::default();
        match row {
            ui::ENV_ROW_FOG_MODE => {
                self.video.fog_mode = defaults.fog_mode;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_FOG_STRENGTH => self.set_fog_strength(defaults.fog_strength),
            ui::ENV_ROW_SUN_SOURCE => self.set_sun_override(false, false),
            ui::ENV_ROW_SUN_VISIBILITY => {
                self.video.sun_visibility = defaults.sun_visibility;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_ENTITY_SUN_LIGHTING => {
                self.video.entity_sun_lighting = defaults.entity_sun_lighting;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_SUN_YAW => {
                let yaw = self
                    .map_sun_editor_values()
                    .map(|v| v.0)
                    .unwrap_or(defaults.sun_yaw);
                self.set_sun_yaw(yaw);
            }
            ui::ENV_ROW_SUN_PITCH => {
                let pitch = self
                    .map_sun_editor_values()
                    .map(|v| v.1)
                    .unwrap_or(defaults.sun_pitch);
                self.set_sun_pitch(pitch);
            }
            ui::ENV_ROW_SUN_INTENSITY => {
                let intensity = self
                    .map_sun_editor_values()
                    .map(|v| v.2)
                    .unwrap_or(defaults.sun_intensity);
                self.set_sun_intensity(intensity);
            }
            ui::ENV_ROW_SUN_COLOR => {
                let color = self
                    .map_sun_editor_values()
                    .map(|v| v.3)
                    .unwrap_or(defaults.sun_color);
                self.set_sun_color(color);
            }
            ui::ENV_ROW_CLOUDS => {
                self.video.clouds = defaults.clouds;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_TYPE => {
                self.video.cloud_type = defaults.cloud_type;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_QUALITY => self.set_cloud_quality(defaults.cloud_quality),
            ui::ENV_ROW_CLOUD_COVERAGE => self.set_cloud_coverage(defaults.cloud_coverage),
            ui::ENV_ROW_CLOUD_HEIGHT => self.set_cloud_height(defaults.cloud_height),
            ui::ENV_ROW_CLOUD_THICKNESS => self.set_cloud_thickness(defaults.cloud_thickness),
            ui::ENV_ROW_WEATHER_WIND_SPEED => {
                self.set_weather_wind_speed(defaults.weather_wind.speed)
            }
            ui::ENV_ROW_WEATHER_WIND_DIRECTION => {
                self.set_weather_wind_direction(defaults.weather_wind.direction)
            }
            ui::ENV_ROW_WEATHER_GUST_STRENGTH => {
                self.set_weather_gust_strength(defaults.weather_wind.gust)
            }
            ui::ENV_ROW_WEATHER_DIRECTION_VARIATION => {
                self.set_weather_direction_variation(defaults.weather_wind.shift)
            }
            ui::ENV_ROW_CLOUD_SHADOWS => {
                self.video.cloud_shadows = defaults.cloud_shadows;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_RENDER_RESOLUTION => {
                self.video.cloud_render_resolution = defaults.cloud_render_resolution;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_TEMPORAL => {
                self.video.cloud_temporal = defaults.cloud_temporal;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_RAIN => {
                self.video.rain = defaults.rain;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_RAIN_INTENSITY => {
                self.video.rain_intensity = defaults.rain_intensity;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_PUDDLE_WATER => {
                self.video.puddle_quality = defaults.puddle_quality;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_PUDDLE_SCATTER => self.set_puddle_scatter(defaults.puddle_scatter),
            ui::ENV_ROW_RAIN_GRADE => self.set_rain_grade(defaults.rain_grade),
            ui::ENV_ROW_FOOTPRINTS => {
                self.video.footprints = defaults.footprints;
                self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                self.footprints_chosen();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_GRASS => {
                self.video.grass = defaults.grass;
                if !self.video.grass || self.applied_grass {
                    self.render_command(RenderCommand::SetGrassEnabled(self.video.grass));
                }
                self.mark_config_dirty();
            }
            ui::ENV_ROW_OCEAN => {
                self.video.ocean = defaults.ocean;
                if !self.video.ocean || self.applied_ocean {
                    self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                }
                self.mark_config_dirty();
            }
            _ => {}
        }
    }

    /// Cloud tuning rows go back through `apply_cloud_tuning`, which owns the
    /// clamping and renderer sync for this block.
    pub(in crate::app::egui_settings) fn reset_cloud_tuning_row(&mut self, row: usize) {
        let defaults = VideoSettings::default();
        match row {
            ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX => {
                if self.video.cloud_temporal_depth_fix != defaults.cloud_temporal_depth_fix {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_SKY_AMBIENT => {
                if self.video.cloud_sky_ambient != defaults.cloud_sky_ambient {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_SHAPE_EVOLUTION => {
                if self.video.cloud_shape_evolution != defaults.cloud_shape_evolution {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_TERRAIN_INTERACTION => {
                if self.video.cloud_terrain_interaction != defaults.cloud_terrain_interaction {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_EMPTY_SKIP => {
                if self.video.cloud_empty_skip != defaults.cloud_empty_skip {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_SHEAR => self.apply_cloud_tuning(row, false, Some(defaults.cloud_shear)),
            ui::CLOUD_ROW_BASE_VARIATION => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_base_variation))
            }
            ui::CLOUD_ROW_AERIAL => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_aerial))
            }
            ui::CLOUD_ROW_HISTORY_BLEND => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_history_blend))
            }
            ui::CLOUD_ROW_MOTION_REJECT => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_motion_reject))
            }
            ui::CLOUD_ROW_THICKNESS_VARIATION => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_thickness_variation))
            }
            ui::CLOUD_ROW_SIZE => self.apply_cloud_tuning(row, false, Some(defaults.cloud_size)),
            ui::CLOUD_ROW_HISTORY_DEPTH_REJECT => {
                if self.video.cloud_history_depth_reject != defaults.cloud_history_depth_reject {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            _ => {}
        }
    }

    pub(in crate::app::egui_settings) fn reset_ocean_global(&mut self, field: u8) {
        let defaults = crate::ocean::OceanSettings::default();
        let settings = &mut self.video.ocean_settings;
        match field {
            OCEAN_MAP_SIZE => settings.map_size = defaults.map_size,
            OCEAN_MESH_QUALITY => settings.mesh_quality = defaults.mesh_quality,
            OCEAN_UPDATES => settings.updates_per_second = defaults.updates_per_second,
            OCEAN_ROUGHNESS => settings.roughness = defaults.roughness,
            OCEAN_NORMAL_STRENGTH => settings.normal_strength = defaults.normal_strength,
            OCEAN_WATER_COLOR => settings.water_color = defaults.water_color,
            OCEAN_FOAM_COLOR => settings.foam_color = defaults.foam_color,
            OCEAN_SEA_SPRAY => settings.sea_spray = defaults.sea_spray,
            OCEAN_WIND_FOAM => settings.wind_foam_streaks = defaults.wind_foam_streaks,
            9 => settings.optics.fog_color = defaults.optics.fog_color,
            10 => settings.optics.fog_distance = defaults.optics.fog_distance,
            11 => settings.optics.transparency = defaults.optics.transparency,
            12 => settings.optics.depth_darkening = defaults.optics.depth_darkening,
            13 => settings.optics.refraction = defaults.optics.refraction,
            14 => settings.optics.caustics = defaults.optics.caustics,
            15 => settings.optics.underwater_cull = defaults.optics.underwater_cull,
            _ => return,
        }
        self.commit_ocean_settings();
    }
}
