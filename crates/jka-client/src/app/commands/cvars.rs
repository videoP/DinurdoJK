//! Commands cvars.
use crate::app::{
    config, event_workers, server_browser, ui, App, ApplyLatchedScope, ClientInfo,
    CloudRenderResolution, ColorLutPreset, ConsoleCvarSetResult, DetailTextureMode, DofQuality,
    Duration, DynamicLightsMode, DynamicShadowsMode, EntityAmbientLightingMode, EntityShadowLight,
    FogMode, FootprintMode, ForcedPlayerModels, FullscreenMode, FxGeometryMode, Ghoul2BatchMode,
    Ghoul2SkinningMode, HudElementId, Instant, OverlayMode, PhysicalPosition,
    PlanarReflectionDebugMode, PuddleQuality, PvsMode, RainIntensity, ReflectionQuality,
    RenderCommand, RendererBackend, SpectatorCameraMode, SunVisibilityMode, TextureFilter,
    VideoSettings, VsyncMode, MAX_SPECTATOR_ORBIT_RANGE, MIN_SPECTATOR_ORBIT_RANGE,
};

impl App {
    pub(in crate::app) fn console_cvar_value(&self, name: &str) -> Option<String> {
        if name.eq_ignore_ascii_case("fs_game") && self.net.is_some() {
            // The server's fs_game is in force; show that, not the local preference.
            return Some(
                self.game
                    .as_ref()
                    .and_then(|dir| dir.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
        if let Some(value) = self.network.cvar_value(name) {
            return Some(value);
        }
        if let Some(value) = self.japro_cg.cvar_value(name) {
            return Some(value);
        }
        let bool_value = |value: bool| if value { "1" } else { "0" }.to_owned();
        Some(match name.to_ascii_lowercase().as_str() {
            "com_maxfps" => self.effective_fps_cap().to_string(),
            "cg_drawfps" => self.video.draw_fps.to_string(),
            "cg_drawtimer" => bool_value(self.video.draw_timer),
            "cg_debugevents" => self.cg_debug_events.to_string(),
            "cg_eventworkers" => bool_value(self.cg_event_workers),
            "cg_asyncassets" => bool_value(crate::asset_jobs::async_enabled()),
            "cg_drawcrosshair" => self.crosshair.style.to_string(),
            "cg_crosshairimage" => self.crosshair.image.to_string(),
            "cg_dynamiccrosshair" => self.crosshair.dynamic.to_string(),
            "cg_crosshairidentifytarget" => bool_value(self.crosshair.identify_target),
            "cg_drawcrosshairnames" => format!("{}", self.crosshair.names),
            "cg_drawcrosshairnamescolours" => bool_value(self.crosshair.names_colours),
            "cg_drawcrosshairnamesopacity" => format!("{}", self.crosshair.names_opacity),
            "cg_drawplayernames" => self.player_names.mode.to_string(),
            "cg_drawplayernamesscale" => format!("{:.3}", self.player_names.scale),
            "cg_crosshairsize" => format!("{:.3}", self.crosshair.size),
            "cg_crosshairstrength" => format!("{:.3}", self.crosshair.strength),
            "cg_crosshaircolor" => {
                let [r, g, b, a] = self.crosshair.color;
                format!("{r} {g} {b} {a}")
            }
            "cg_movementkeys" => self.movement_keys_hud.mode.to_string(),
            "cg_movementkeysx" => format!("{:.3}", self.movement_keys_hud.x),
            "cg_movementkeysy" => format!("{:.3}", self.movement_keys_hud.y),
            "cg_movementkeyssize" => format!("{:.3}", self.movement_keys_hud.size),
            "cg_movementkeyswalk" => bool_value(self.movement_keys_hud.walk),
            "cg_strafehelper" => self.strafe_helper.flags.to_string(),
            "cg_strafehelper_fps" => format!("{:.3}", self.strafe_helper.fps),
            "cg_strafehelperoffset" => format!("{:.3}", self.strafe_helper.offset),
            "cg_strafehelperlinewidth" => format!("{:.3}", self.strafe_helper.line_width),
            "cg_strafehelperprecision" => self.strafe_helper.precision.to_string(),
            "cg_strafehelpercutoff" => format!("{:.3}", self.strafe_helper.cutoff),
            "cg_strafehelperactivecolor" => {
                let [r, g, b, a] = self.strafe_helper.active_color;
                format!("{r} {g} {b} {a}")
            }
            "cg_strafehelperinactivealpha" => self.strafe_helper.inactive_alpha.to_string(),
            "cg_strafetrailradius" => format!("{:.3}", self.strafe_trails.settings.radius),
            "cg_strafetraillife" => format!("{:.3}", self.strafe_trails.settings.life_seconds),
            "cg_strafetrailfps" => format!("{:.3}", self.strafe_trails.settings.fps),
            "cg_strafetrailplums" => bool_value(self.strafe_trails.settings.plums),
            "cg_strafetrailghost" => bool_value(self.strafe_trails.settings.ghost),
            "cg_strafetrailplayers" => self.strafe_trails.settings.players.to_string(),
            "cg_logstrafetrail" => self.strafe_trails.settings.log_name.clone(),
            "cg_strafetraildistance" => format!("{:.1}", self.strafe_trails.settings.draw_distance),
            "cg_rghostalpha" => format!("{:.3}", self.race_ghost_alpha),
            "cg_rghostname" => bool_value(self.race_ghost_name),
            "cg_rghosttrail" => bool_value(self.race_ghost_trail),
            "cg_rghostvelocitydelta" => bool_value(self.race_ghost_velocity_delta),
            "cg_rghostdistancedelta" => bool_value(self.race_ghost_distance_delta),
            "cg_rghostdemobaseurl" => self.race_ghost_demo_base_url.clone(),
            hud_cvar if HudElementId::from_cvar(hud_cvar).is_some() => {
                HudElementId::from_cvar(hud_cvar)
                    .map(|id| self.hud_layout.element(id).to_config())
                    .unwrap_or_default()
            }
            "cg_hudsnap" => bool_value(self.hud_layout.snap_to_grid),
            "cg_hudgridsize" => format!("{:.3}", self.hud_layout.grid_size),
            "model" => self.solo_client_info.model_cvar(),
            "cg_forcemodel" => self.force_model.clone(),
            "con_timestamps" => bool_value(self.console_timestamps),
            "con_suggest" => bool_value(self.console_suggest),
            "cg_chatboxcompletion" => bool_value(self.chatbox_completion),
            "cl_chatlog" => bool_value(self.chat_log_enabled),
            "ui_vgs" => self.ui_vgs.to_string(),
            "r_jumpheightshade" => bool_value(self.jump_height_shade),
            "cg_screenshake" => self.screen_shake.to_string(),
            "cg_zoomfov" => format!("{:.3}", self.japro_zoom_fov),
            "cg_fkduration" => self.japro_fk_duration.to_string(),
            "cg_fkfirstjumpduration" => self.japro_fk_first_jump_duration.to_string(),
            "cg_fksecondjumpdelay" => self.japro_fk_second_jump_delay.to_string(),
            "sv_master1" => self.server_browser.master_servers[0].clone(),
            "sv_master2" => self.server_browser.master_servers[1].clone(),
            "sv_master3" => self.server_browser.master_servers[2].clone(),
            "sv_master4" => self.server_browser.master_servers[3].clone(),
            "sv_master5" => self.server_browser.master_servers[4].clone(),
            "sensitivity" => format!("{:.6}", self.mouse_input.sensitivity),
            "m_yaw" => format!("{:.6}", self.mouse_input.yaw),
            "m_pitch" => format!("{:.6}", self.mouse_input.pitch),
            "cl_mouseaccel" => format!("{:.6}", self.mouse_input.accel),
            "cg_fov" => format!("{:.3}", self.camera.cg_fov()),
            "s_volume" => format!("{:.3}", self.audio.effects_volume),
            "s_volumevoice" => format!("{:.3}", self.audio.voice_volume),
            "s_musicvolume" => format!("{:.3}", self.audio.music_volume),
            "s_separation" => format!("{:.3}", self.audio.separation),
            "s_mutewhenunfocused" => bool_value(self.audio.mute_when_unfocused),
            "s_steamaudio" => bool_value(self.audio.steam_audio),
            "s_steamaudiobinaural" => bool_value(self.audio.steam_audio_binaural),
            "s_steamaudioenvironmental" => bool_value(self.audio.steam_audio_environmental),
            "cg_jumpsounds" => self.audio.game.jump.to_string(),
            "cg_rollsounds" => self.audio.game.roll.to_string(),
            "cg_notaunt" => bool_value(self.audio.game.no_taunt),
            "cg_duelsounds" => self.audio.game.duel.to_string(),
            "cg_killsounds" => self.audio.game.kill.to_string(),
            "cg_killmessage" => self.audio.game.kill_message.to_string(),
            "cg_drawrewards" => self.audio.game.draw_rewards.to_string(),
            "cg_hitsounds" => self.audio.game.hit.to_string(),
            "cg_duelmusic" => bool_value(self.audio.game.duel_music),
            "cg_ambientsounds" => bool_value(self.audio.game.ambient),
            "cg_blood" => self.audio.game.blood.to_string(),
            "cg_autoswitch" => self.audio.game.auto_switch.to_string(),
            "cg_scoreplums" => bool_value(self.audio.game.score_plums),
            "cg_ghoul2marks" => self.audio.game.g2_marks.to_string(),
            "cg_racesounds" => self.audio.game.race_sounds.to_string(),
            "cg_chatsounds" => self.audio.game.chat_sounds.to_string(),
            "cg_footsteps" => self.audio.game.footsteps.to_string(),
            "cg_thirdperson" => bool_value(self.third_person.enabled),
            "cg_speccamera" => self.spectator_camera.mode.as_i32().to_string(),
            "cg_speccameramotion" => bool_value(self.spectator_camera.motion_direction),
            "cg_specorbitrange" => format!("{:.3}", self.spectator_camera.orbit_range),
            "cg_fpls" => bool_value(self.first_person_lightsaber),
            "cg_sabertrail" => self.saber_trail.to_string(),
            "cg_saberteamcolors" => bool_value(self.saber_team_colors),
            "cg_saberstaffmulticolor" => bool_value(self.saber_staff_multi_color),
            "cg_drawteamoverlay" => self.team_overlay.mode.to_string(),
            "cg_drawteamoverlayx" => self.team_overlay.x.to_string(),
            "cg_drawteamoverlayy" => self.team_overlay.y.to_string(),
            "cg_drawteamoverlayweapons" => bool_value(self.team_overlay.weapons),
            "cg_drawteamoverlayscale" => format!("{:.3}", self.team_overlay.scale),
            "cg_drawteamoverlaymaxhp" => format!("{:.3}", self.team_overlay.max_hp),
            "cg_drawteamoverlayforce" => bool_value(self.team_overlay.force),
            "cg_scoredeaths" => self.score_deaths.to_string(),
            "cg_drawscores" => self.draw_scores.to_string(),
            "cg_fxfps" => self.video.fx_fps.to_string(),
            "cg_fxfpsscope" => self.video.fx_fps_scope.to_string(),
            "fx_physics" => self.video.fx_physics.to_string(),
            "fx_lod" => self.video.fx_lod.to_string(),
            "r_fxlodscale" => self.video.fx_lod_scale.to_string(),
            "r_lodscale" => self.video.lod_scale.to_string(),
            "fx_countscale" => self.video.fx_count_scale.to_string(),
            "r_fxgeometry" => self.video.fx_geometry.config_value().to_owned(),
            "r_fxzeroalphadiscard" => bool_value(self.video.fx_zero_alpha_discard),
            "cg_thirdpersonalpha" => format!("{:.3}", self.third_person.alpha),
            "cg_thirdpersonangle" => format!("{:.3}", self.third_person.angle),
            "cg_thirdpersoncameradamp" => format!("{:.3}", self.third_person.camera_damp),
            "cg_thirdpersonhorzoffset" => format!("{:.3}", self.third_person.horz_offset),
            "cg_thirdpersonpitchoffset" => format!("{:.3}", self.third_person.pitch_offset),
            "cg_thirdpersonrange" => format!("{:.3}", self.third_person.range),
            "cg_thirdpersonspecialcam" => bool_value(self.third_person.special_cam),
            "cg_thirdpersontargetdamp" => format!("{:.3}", self.third_person.target_damp),
            "cg_thirdpersonvertoffset" => format!("{:.3}", self.third_person.vert_offset),
            "pmove_msec" => self.video.physics_msec.to_string(),
            "cl_timerresolution1ms" => bool_value(self.video.timer_resolution_1ms),
            "cl_input_latelatch" => bool_value(self.video.input_latelatch),
            "r_physics" => bool_value(self.video.client_physics),
            "r_physicshz" => self.video.client_physics_hz.to_string(),
            "r_physicsmaxsubsteps" => self.video.client_physics_max_substeps.to_string(),
            "r_physicsccd" => bool_value(self.video.client_physics_ccd),
            "r_physicssleeping" => bool_value(self.video.client_physics_sleeping),
            "r_ragdolls" => bool_value(self.video.ragdolls),
            "r_ragdollmax" => self.video.ragdoll_max.to_string(),
            "r_ragdolllifetime" => format!("{:.1}", self.video.ragdoll_lifetime),
            "r_ragdollselfcollision" => bool_value(self.video.ragdoll_self_collision),
            "r_jigglephysics" => bool_value(self.video.jiggle_physics),
            "r_jigglesolver" => self.video.jiggle_solver.to_string(),
            "r_jigglestrength" => format!("{:.3}", self.video.jiggle_strength),
            "r_jigglebreaststrength" => format!("{:.3}", self.video.jiggle_breast_strength),
            "r_jiggleglutestrength" => format!("{:.3}", self.video.jiggle_glute_strength),
            "r_jigglestiffness" => format!("{:.3}", self.video.jiggle_stiffness),
            "r_jiggledamping" => format!("{:.3}", self.video.jiggle_damping),
            "r_jiggleglutelift" => format!("{:.3}", self.video.jiggle_glute_lift),
            "r_jigglejpstiffness" => format!("{:.3}", self.video.jiggle_jp_stiffness),
            "r_jigglejpdrag" => format!("{:.3}", self.video.jiggle_jp_drag),
            "r_jigglejpairdrag" => format!("{:.3}", self.video.jiggle_jp_air_drag),
            "r_jigglejpstretch" => format!("{:.3}", self.video.jiggle_jp_stretch),
            "r_jigglejpsoften" => format!("{:.3}", self.video.jiggle_jp_soften),
            "r_jigglejpgravity" => format!("{:.3}", self.video.jiggle_jp_gravity),
            "cg_dismember" => self.video.dismemberment.to_string(),
            "r_dismembermax" => self.video.dismember_max.to_string(),
            "r_dismemberlifetime" => format!("{:.1}", self.video.dismember_lifetime),
            "r_clothphysics" => bool_value(self.video.cloth_physics),
            "r_clothbodycollision" => bool_value(self.video.cloth_body_collision),
            "r_clothbodyclearance" => self.video.cloth_body_clearance.to_string(),
            "r_clothairresistance" => self.video.cloth_air_resistance.to_string(),
            "r_clothturnresponse" => self.video.cloth_turn_response.to_string(),
            "r_clothanimationinfluence" => self.video.cloth_animation_influence.to_string(),
            "r_clothwind" => bool_value(self.video.cloth_wind),
            "r_physicsprops" => bool_value(self.video.physics_props),
            "r_physicspropmax" => self.video.physics_prop_max.to_string(),
            "r_physicsdebris" => bool_value(self.video.physics_debris),
            "r_physicsdebrismax" => self.video.physics_debris_max.to_string(),
            "r_physicsdebrislifetime" => format!("{:.1}", self.video.physics_debris_lifetime),
            "r_physicsplayerpush" => bool_value(self.video.physics_player_push),
            "r_physicsweaponimpulses" => bool_value(self.video.physics_weapon_impulses),
            "r_physicsexplosionimpulses" => bool_value(self.video.physics_explosion_impulses),
            "r_physicsforceimpulses" => bool_value(self.video.physics_force_impulses),
            "r_physicsdebug" => bool_value(self.video.physics_debug_draw),
            "r_physicsstats" => bool_value(self.video.physics_stats),
            "r_fullscreen" => self.video.fullscreen.config_value().to_string(),
            "r_backend" => self.video.renderer_backend.config_value().to_owned(),
            "r_swapinterval" => self.video.vsync.config_value().to_string(),
            "r_maxframelatency" => self.video.max_frame_latency.to_string(),
            "r_ext_multisample" => {
                if self.video.msaa_samples <= 1 {
                    "0".into()
                } else {
                    self.video.msaa_samples.to_string()
                }
            }
            "r_texturemode" => match self.video.texture_filter {
                TextureFilter::Nearest => "GL_NEAREST".into(),
                TextureFilter::Bilinear => "GL_LINEAR_MIPMAP_NEAREST".into(),
                _ => "GL_LINEAR_MIPMAP_LINEAR".into(),
            },
            "r_picmip" => self.video.picmip.to_string(),
            "r_ext_texture_filter_anisotropic" => match self.video.texture_filter {
                TextureFilter::Anisotropic2x => "2".into(),
                TextureFilter::Anisotropic4x => "4".into(),
                TextureFilter::Anisotropic8x => "8".into(),
                TextureFilter::Anisotropic16x => "16".into(),
                _ => "0".into(),
            },
            "r_detailtextures" => self.video.detail_textures.config_value().into(),
            "r_detailtexture" => "auto".into(),
            "r_detailtexturefade" => bool_value(self.video.detail_texture_fade),
            "r_detailtexturefadedistance" => {
                format!("{:.0}", self.video.detail_texture_fade_distance)
            }
            "r_customwidth" => self.video.resolution[0].to_string(),
            "r_customheight" => self.video.resolution[1].to_string(),
            "r_windowx" => self
                .video
                .window_position
                .map_or_else(|| "auto".into(), |p| p[0].to_string()),
            "r_windowy" => self
                .video
                .window_position
                .map_or_else(|| "auto".into(), |p| p[1].to_string()),
            "r_windowmaximized" => bool_value(self.video.window_maximized),
            "r_gamma" => format!("{:.3}", self.video.gamma),
            "r_gammamethod" => self.video.gamma_method.config_value().to_owned(),
            "r_modelbrightness" => format!("{:.3}", self.video.model_brightness),
            "r_modelbrightnesslock" => bool_value(self.video.model_brightness_locked),
            "r_dynamiclightbrightness" => format!("{:.3}", self.video.dynamic_light_brightness),
            "r_dynamiclightbrightnesslock" => {
                bool_value(self.video.dynamic_light_brightness_locked)
            }
            "r_drawmapmodels" => bool_value(self.video.draw_map_models),
            "r_drawtriggers" => bool_value(self.video.draw_triggers),
            "r_drawclipbrushes" => bool_value(self.video.draw_clip_brushes),
            "r_drawentities" => bool_value(self.video.draw_entities),
            "r_hdr" => bool_value(self.video.hdr),
            "r_floatlightmap" => bool_value(self.video.float_lightmap),
            "r_tonemap" => bool_value(self.video.tone_mapping),
            "r_autoexposure" => bool_value(self.video.auto_exposure),
            "r_bloom" => bool_value(self.video.bloom),
            "r_halation" => bool_value(self.video.halation),
            "r_ssao" => bool_value(self.video.ssao),
            "r_staticbspao" => bool_value(self.video.static_bsp_ao),
            "r_staticbspaomode" => {
                if self.video.static_bsp_ao_lightmap {
                    "lightmap".into()
                } else {
                    "vertex".into()
                }
            }
            "r_staticbspaosamples" => self.video.static_bsp_ao_samples.to_string(),
            "r_staticbspaoresolution" => self.video.static_bsp_ao_resolution.to_string(),
            "r_staticbspaostrength" => self.video.static_bsp_ao_strength.to_string(),
            "r_staticbspaorange" => self.video.static_bsp_ao_range.to_string(),
            "r_staticbspaocurrentcell" => bool_value(self.video.static_bsp_ao_current_cell),
            "r_fxaa" => bool_value(self.video.fxaa),
            "r_smaa" => bool_value(self.video.smaa),
            "r_taa" => bool_value(self.video.taa),
            "r_contactshadows" => bool_value(self.video.contact_shadows),
            "r_weatherwind" => format!(
                "{:.2} {:.1} {:.3} {:.1}",
                self.video.weather_wind.speed,
                self.video.weather_wind.direction,
                self.video.weather_wind.gust,
                self.video.weather_wind.shift,
            ),
            "r_cloudshadows" => bool_value(self.video.cloud_shadows),
            "r_cloudrenderresolution" => {
                self.video.cloud_render_resolution.config_value().to_owned()
            }
            "r_cloudtemporal" => bool_value(self.video.cloud_temporal),
            "r_cloudtemporaldepthfix" => bool_value(self.video.cloud_temporal_depth_fix),
            "r_cloudshapeevolution" => bool_value(self.video.cloud_shape_evolution),
            "r_cloudterraininteraction" => bool_value(self.video.cloud_terrain_interaction),
            "r_cloudemptyskip" => bool_value(self.video.cloud_empty_skip),
            "r_rain" => bool_value(self.video.rain),
            "r_rainintensity" => self.video.rain_intensity.config_value().to_owned(),
            "r_puddlequality" => self.video.puddle_quality.config_value().to_owned(),
            "r_puddlescatter" => format!("{:.3}", self.video.puddle_scatter),
            "r_raingrade" => format!("{:.3}", self.video.rain_grade),
            "r_footprints" => self.video.footprints.config_value().to_owned(),
            "r_grass" => bool_value(self.video.grass),
            "r_clouds" => bool_value(self.video.clouds),
            "r_cloudquality" => format!("{:.3}", self.video.cloud_quality),
            "r_mode" => "-1".into(),
            "r_cloudtype" => self.video.cloud_type.config_value().to_owned(),
            "r_cloudcoverage" => format!("{:.3}", self.video.cloud_coverage),
            "r_cloudheight" => format!("{:.1}", self.video.cloud_height),
            "r_cloudthickness" => format!("{:.1}", self.video.cloud_thickness),
            "r_cloudshear" => format!("{:.3}", self.video.cloud_shear),
            "r_cloudbasevariation" => format!("{:.3}", self.video.cloud_base_variation),
            "r_cloudaerial" => format!("{:.3}", self.video.cloud_aerial),
            "r_cloudskyambient" => bool_value(self.video.cloud_sky_ambient),
            "r_cloudhistoryblend" => format!("{:.3}", self.video.cloud_history_blend),
            "r_cloudmotionreject" => format!("{:.3}", self.video.cloud_motion_reject),
            "r_cloudhistorydepthreject" => bool_value(self.video.cloud_history_depth_reject),
            "r_cloudthicknessvariation" => format!("{:.3}", self.video.cloud_thickness_variation),
            "r_cloudsize" => format!("{:.3}", self.video.cloud_size),
            "r_oceanroughness" => format!("{:.3}", self.video.ocean_settings.roughness),
            "r_oceannormalstrength" => format!("{:.3}", self.video.ocean_settings.normal_strength),
            "r_oceanwatercolor" => {
                let c = self.video.ocean_settings.water_color;
                format!("{:.4} {:.4} {:.4}", c[0], c[1], c[2])
            }
            "r_oceanfoamcolor" => {
                let c = self.video.ocean_settings.foam_color;
                format!("{:.4} {:.4} {:.4}", c[0], c[1], c[2])
            }
            "r_oceancascade1" | "r_oceancascade2" | "r_oceancascade3" => {
                let digit = name.as_bytes()[name.len() - 1];
                let c = self.video.ocean_settings.cascades[usize::from(digit - b'1')];
                format!(
                    "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                    c.tile_length[0],
                    c.tile_length[1],
                    c.displacement_scale,
                    c.normal_scale,
                    c.wind_speed,
                    c.wind_direction,
                    c.fetch_length,
                    c.swell,
                    c.spread,
                    c.detail,
                    c.whitecap,
                    c.foam_amount,
                )
            }
            "r_oceanauthoring" => {
                let a = self.video.ocean_settings.authored;
                format!(
                    "{} {} {} {} {} {} {} {} {} {} {}",
                    a.amplitude,
                    a.wavelength,
                    a.speed,
                    a.direction,
                    a.steepness,
                    a.slosh,
                    a.wind_chop,
                    a.foam,
                    a.foam_lifetime,
                    a.spray,
                    a.seed,
                )
            }
            "r_ocean" => bool_value(self.video.ocean),
            "r_oceanmapsize" => self.video.ocean_settings.map_size.to_string(),
            "r_oceanmeshquality" => self.video.ocean_settings.mesh_quality.to_string(),
            "r_oceanupdates" => format!("{:.3}", self.video.ocean_settings.updates_per_second),
            "r_oceanseaspray" => bool_value(self.video.ocean_settings.sea_spray),
            "r_oceanwindfoam" => bool_value(self.video.ocean_settings.wind_foam_streaks),
            "r_oceanfogcolor" => {
                let c = self.video.ocean_settings.optics.fog_color;
                format!("{} {} {}", c[0], c[1], c[2])
            }
            "r_oceanfogdistance" => self.video.ocean_settings.optics.fog_distance.to_string(),
            "r_oceantransparency" => self.video.ocean_settings.optics.transparency.to_string(),
            "r_oceandepthdarkening" => self.video.ocean_settings.optics.depth_darkening.to_string(),
            "r_oceanrefraction" => self.video.ocean_settings.optics.refraction.to_string(),
            "r_oceancaustics" => self.video.ocean_settings.optics.caustics.to_string(),
            "r_oceanunderwatercull" => self.video.ocean_settings.optics.underwater_cull.to_string(),
            "r_grassprecompute" => bool_value(self.video.grass_precompute),
            "r_grassmidlod" => bool_value(self.video.grass_mid_lod),
            "r_grassfronttoback" => bool_value(self.video.grass_front_to_back),
            "r_contactshadowdebug" => self.video.contact_shadow_debug.to_string(),
            "r_drawfog" => self.video.fog_mode.drawfog_value().to_string(),
            "r_fogstrength" => format!("{:.3}", self.video.fog_strength),
            "r_sunoverride" => bool_value(self.video.sun_override),
            "r_sunvisibility" => self.video.sun_visibility.config_value().to_owned(),
            "r_entitysunlighting" => bool_value(self.video.entity_sun_lighting),
            "r_sunyaw" => format!("{:.3}", self.video.sun_yaw),
            "r_sunpitch" => format!("{:.3}", self.video.sun_pitch),
            "r_sunintensity" => format!("{:.3}", self.video.sun_intensity),
            "r_suncolor" => format!(
                "{:.4} {:.4} {:.4}",
                self.video.sun_color[0], self.video.sun_color[1], self.video.sun_color[2]
            ),
            "r_distancecullscale" => format!("{:.3}", self.video.distance_cull_scale),
            "r_reflectionquality" => self.video.reflection_quality.config_value().to_owned(),
            "r_reflectiondebug" => bool_value(self.video.reflection_debug),
            "r_chromaticaberration" => format!("{:.3}", self.video.chromatic_aberration),
            "r_vignette" => bool_value(self.video.vignette),
            "r_filmgrain" => format!("{:.3}", self.video.film_grain_strength),
            "r_motionblur" => format!("{:.3}", self.video.motion_blur_strength),
            "r_depthoffield" => format!("{:.3}", self.video.depth_of_field_strength),
            "r_dofautofocus" => bool_value(self.video.dof_autofocus),
            "r_dofquality" => self.video.dof_quality.config_value().to_owned(),
            "r_splittoning" => u8::from(self.video.split_toning.enabled).to_string(),
            "r_splittoningstrength" => format!("{:.3}", self.video.split_toning.strength),
            "r_splittoningshadowhue" => format!("{:.3}", self.video.split_toning.shadow_hue),
            "r_splittoningshadowsaturation" => format!("{:.3}", self.video.split_toning.shadow_saturation),
            "r_splittoninghighlighthue" => format!("{:.3}", self.video.split_toning.highlight_hue),
            "r_splittoninghighlightsaturation" => format!("{:.3}", self.video.split_toning.highlight_saturation),
            "r_splittoningbalance" => format!("{:.3}", self.video.split_toning.balance),
            "r_colorlut" => self.video.color_lut.config_value().to_owned(),
            "r_colorlutstrength" => format!("{:.3}", self.video.color_lut_strength),
            "r_gpudriven" => bool_value(self.video.gpu_driven),
            "r_hizocclusion" => bool_value(self.video.hiz_occlusion),
            "r_entityambientlighting" => {
                self.video.entity_ambient_lighting.config_value().to_owned()
            }
            "r_dynamiclights" => self.video.dynamic_lights.config_value().to_owned(),
            "r_rtsamples" => self.video.rt_samples.to_string(),
            "r_dynamiclightfalloff" => self.video.dynamic_light_falloff.to_string(),
            "r_rtresolution" => if self.video.rt_half_resolution {
                "half"
            } else {
                "full"
            }
            .to_owned(),
            "r_maplightsimulation" => bool_value(self.video.map_light_simulation),
            "r_fullbright" => bool_value(!self.video.world_lighting),
            "r_vertexlight" => bool_value(self.video.vertex_lighting),
            "r_lightmap" => bool_value(self.video.lightmap_only),
            "r_modernsabers" => bool_value(self.video.modern_sabers),
            "r_flares" => bool_value(self.video.flares),
            "r_saberimpactfx" => bool_value(self.video.saber_impact_fx),
            "r_sabermarks" => self.video.saber_marks.config_value().to_owned(),
            "r_dynamicshadows" => self.video.dynamic_shadows.config_value().to_owned(),
            "r_emissivearealights" => bool_value(self.video.emissive_area_lights),
            "r_voxelprobegi" => bool_value(self.video.voxel_probe_gi),
            "r_entityshadowlight" => self.video.entity_shadow_light.config_value().to_owned(),
            "r_locallightshadows" => bool_value(self.video.local_light_shadows),
            "r_pbr" => bool_value(self.video.pbr),
            "fs_allowassetoverrides" => bool_value(self.video.allow_asset_overrides),
            "r_gennormalmaps" => bool_value(self.video.gen_normal_maps),
            "r_deluxemapping" => bool_value(self.video.deluxe_mapping),
            "r_deluxespecular" => format!("{:.3}", self.video.deluxe_specular),
            "r_cascadedshadows" => bool_value(self.video.cascaded_shadows),
            "r_planardebug" => self
                .video
                .planar_reflection_debug
                .label()
                .to_ascii_lowercase(),
            "r_showtris" => self.video.wireframe_mask.to_string(),
            "r_skipui" => bool_value(self.video.skip_ui),
            "developer" => self.video.developer_level.to_string(),
            "r_verbose" => self.video.renderer_verbose.to_string(),
            "r_perftrace" => bool_value(self.video.perf_trace),
            "r_pom" => bool_value(self.video.pom),
            "r_worldpath" => if self.video.force_unified_world {
                "unified"
            } else {
                "auto"
            }
            .to_owned(),
            "r_gputimings" => bool_value(self.video.gpu_timings),
            "r_ghoul2skinning" => self.video.ghoul2_skinning.config_value().to_owned(),
            "r_ghoul2earlycull" => bool_value(self.video.ghoul2_early_cull),
            "r_lodbias" => self.video.ghoul2_lod_bias.to_string(),
            "r_ghoul2batchdraws" => self.video.ghoul2_batch_draws.config_value().to_owned(),
            "r_ghoul2animsmooth" => self.video.ghoul2_anim_smooth.to_string(),
            "r_novis" => bool_value(self.video.pvs_mode == PvsMode::Off),
            "r_pvsmode" => self.video.pvs_mode.config_value().to_owned(),
            _ => return None,
        })
    }

    pub(in crate::app) fn parse_console_bool(value: &str) -> Option<bool> {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "true" | "yes" => Some(true),
            "0" | "off" | "false" | "no" => Some(false),
            _ => None,
        }
    }

    pub(in crate::app) fn normalize_latched_console_value(
        &self,
        name: &str,
        value: &str,
    ) -> Result<String, String> {
        let lower = name.to_ascii_lowercase();
        let boolean = || {
            Self::parse_console_bool(value)
                .map(|enabled| if enabled { "1" } else { "0" }.to_owned())
                .ok_or_else(|| format!("{name}: expected 0/1, off/on, false/true"))
        };
        match lower.as_str() {
            "r_pbr" | "r_gennormalmaps" | "r_floatlightmap" | "fs_allowassetoverrides" => boolean(),
            "r_picmip" => value
                .trim()
                .parse::<u32>()
                .map(|picmip| picmip.min(16).to_string())
                .map_err(|_| format!("{name}: expected an integer from 0 to 16")),
            "r_fullscreen" => FullscreenMode::from_config(value)
                .map(|mode| mode.config_value().to_string())
                .ok_or_else(|| {
                    format!("{name}: expected 0 (windowed), 1 (borderless), or 2 (exclusive)")
                }),
            "r_backend" => RendererBackend::from_config(value)
                .map(|backend| backend.config_value().to_owned())
                .ok_or_else(|| format!("{name}: expected vulkan or dx12")),
            "r_customwidth" => value
                .trim()
                .parse::<u32>()
                .map(|pixels| pixels.max(320).to_string())
                .map_err(|_| format!("{name}: expected an integer pixel size")),
            "r_customheight" => value
                .trim()
                .parse::<u32>()
                .map(|pixels| pixels.max(240).to_string())
                .map_err(|_| format!("{name}: expected an integer pixel size")),
            _ => Ok(value.trim().to_owned()),
        }
    }

    pub(in crate::app) fn set_console_cvar_from_console(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<ConsoleCvarSetResult, String> {
        let entry =
            crate::console::find(name).ok_or_else(|| format!("Cvar {name} does not exist."))?;
        if entry.latch == crate::console::LatchScope::None {
            self.set_console_cvar(name, value)?;
            return Ok(ConsoleCvarSetResult::Applied);
        }

        let normalized = self.normalize_latched_console_value(entry.name, value)?;
        let key = entry.name.to_ascii_lowercase();
        let current = self
            .console_cvar_value(entry.name)
            .ok_or_else(|| format!("{}: cvar is not writable here", entry.name))?;

        // OpenJK Cvar_Set2 cancels a pending latch when the requested value is
        // set back to the current live value.
        if normalized.eq_ignore_ascii_case(&current) {
            let removed = self.latched_console_cvars.remove(&key).is_some();
            if removed {
                self.mark_config_dirty();
            }
            return Ok(ConsoleCvarSetResult::Unchanged);
        }
        if self
            .latched_console_cvars
            .get(&key)
            .is_some_and(|pending| pending.eq_ignore_ascii_case(&normalized))
        {
            return Ok(ConsoleCvarSetResult::Unchanged);
        }

        self.latched_console_cvars.insert(key, normalized);
        self.mark_config_dirty();
        Ok(ConsoleCvarSetResult::Latched)
    }

    pub(in crate::app) fn apply_latched_console_cvars(&mut self, scope: ApplyLatchedScope) {
        let ready: Vec<_> = self
            .latched_console_cvars
            .iter()
            .filter_map(|(name, value)| {
                let entry = crate::console::find(name)?;
                let apply = match (scope, entry.latch) {
                    (
                        ApplyLatchedScope::MapLoad,
                        crate::console::LatchScope::MapLoadOrVidRestart,
                    ) => true,
                    (
                        ApplyLatchedScope::VidRestart,
                        crate::console::LatchScope::VidRestart
                        | crate::console::LatchScope::MapLoadOrVidRestart,
                    ) => true,
                    _ => false,
                };
                apply.then(|| (entry.name.to_owned(), value.clone()))
            })
            .collect();

        for (name, value) in ready {
            self.latched_console_cvars
                .remove(&name.to_ascii_lowercase());
            if let Err(error) = self.set_console_cvar(&name, &value) {
                self.push_console_line(format!("^1{error}"));
            }
        }
    }

    pub(in crate::app) fn apply_latched_values_to_settings(&self, settings: &mut VideoSettings) {
        for (name, value) in &self.latched_console_cvars {
            match name.as_str() {
                "r_fullscreen" => {
                    if let Some(mode) = FullscreenMode::from_config(value) {
                        settings.fullscreen = mode;
                    }
                }
                "r_backend" => {
                    if let Some(backend) = RendererBackend::from_config(value) {
                        settings.renderer_backend = backend;
                    }
                }
                "r_customwidth" => {
                    if let Ok(width) = value.parse::<u32>() {
                        settings.resolution[0] = width.max(320);
                    }
                }
                "r_customheight" => {
                    if let Ok(height) = value.parse::<u32>() {
                        settings.resolution[1] = height.max(240);
                    }
                }
                "r_pbr" => settings.pbr = Self::parse_console_bool(value).unwrap_or(settings.pbr),
                "fs_allowassetoverrides" => {
                    settings.allow_asset_overrides =
                        Self::parse_console_bool(value).unwrap_or(settings.allow_asset_overrides)
                }
                "r_gennormalmaps" => {
                    settings.gen_normal_maps =
                        Self::parse_console_bool(value).unwrap_or(settings.gen_normal_maps)
                }
                "r_floatlightmap" => {
                    settings.float_lightmap =
                        Self::parse_console_bool(value).unwrap_or(settings.float_lightmap)
                }
                "r_picmip" => {
                    if let Ok(picmip) = value.parse::<u32>() {
                        settings.picmip = picmip.min(16);
                    }
                }
                _ => {}
            }
        }
    }

    pub(in crate::app) fn set_console_cvar(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<(), String> {
        if let Some(result) = self.japro_cg.set_cvar(name, value) {
            result?;
            if let Some(session) = self.game_session.as_mut() {
                session.apply_client_options(
                    self.screen_shake,
                    self.audio.game,
                    self.video.footprints,
                    self.japro_cg,
                    self.network.plugin_disable,
                );
            }
            self.mark_config_dirty();
            self.publish_snapshot();
            self.publish_ui();
            return Ok(());
        }
        if let Some(result) = self.network.set_cvar(name, value) {
            let userinfo_changed = result?;
            self.mark_config_dirty();
            let lower = name.to_ascii_lowercase();
            if lower == "cp_plugindisable" {
                if let Some(session) = self.game_session.as_mut() {
                    session
                        .client_game
                        .set_plugin_disable(self.network.plugin_disable);
                }
                // The profile preview also uses the same local black-saber clamp.
                self.profile_preview_key = None;
            }
            if lower.starts_with("char_color_") {
                let rgb = self.network.char_color;
                if let Some(server) = self.local_server.as_mut() {
                    server.set_char_color(rgb);
                }
                self.profile_preview_key = None;
            }
            // jaPRO advertises these two through cg_displayNetSettings.
            let net_display_changed = matches!(lower.as_str(), "cl_maxpackets" | "cl_timenudge")
                && self.refresh_japro_userinfo_state();
            if userinfo_changed || net_display_changed {
                self.request_userinfo_send();
            }
            if matches!(
                lower.as_str(),
                "cg_predictiondebug" | "cg_predictionmisshighlight" | "cg_predictionmissthreshold"
            ) {
                self.publish_transient_ui();
            }
            return Ok(());
        }
        if name.eq_ignore_ascii_case("model") {
            let result = self.set_console_cvar_inner(name, value);
            if result.is_ok() {
                self.request_userinfo_send();
            }
            return result;
        }
        let result = self.set_console_cvar_inner(name, value);
        // ...and these through cg_displayCameraPosition / cg_displayNetSettings.
        if result.is_ok()
            && matches!(
                name.to_ascii_lowercase().as_str(),
                "cg_thirdperson"
                    | "cg_thirdpersonrange"
                    | "cg_thirdpersonvertoffset"
                    | "com_maxfps"
            )
            && self.refresh_japro_userinfo_state()
        {
            self.request_userinfo_send();
        }
        result
    }

    pub(in crate::app) fn set_console_cvar_inner(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<(), String> {
        let lower = name.to_ascii_lowercase();
        let boolean = || {
            Self::parse_console_bool(value)
                .ok_or_else(|| format!("{name}: expected 0/1, off/on, false/true"))
        };
        let finite_number = || {
            value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|number| number.is_finite())
                .ok_or_else(|| format!("{name}: expected a finite number"))
        };
        match lower.as_str() {
            "con_timestamps" => {
                self.console_timestamps = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "con_suggest" => {
                self.console_suggest = boolean()?;
                self.console_suggest_index = 0;
                self.console_suggest_picked = false;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_chatboxcompletion" => {
                self.chatbox_completion = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cl_chatlog" => {
                let enabled = boolean()?;
                if self.chat_log_enabled != enabled {
                    self.chat_log_enabled = enabled;
                    if enabled {
                        self.begin_chat_log_for_current_connection();
                        if let Some(map_name) = self
                            .game_session
                            .as_ref()
                            .filter(|session| session.live && !session.local)
                            .and_then(|session| session.map_name.clone())
                        {
                            self.update_chat_log_live_metadata(&map_name);
                        }
                    } else {
                        self.chat_log.end("Chat logging disabled");
                    }
                    self.mark_config_dirty();
                    self.publish_ui();
                }
            }
            "ui_vgs" => {
                self.ui_vgs = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| format!("{name}: expected an integer"))?;
                self.mark_config_dirty();
                if self.ui_vgs == 0 && self.overlay == OverlayMode::Vgs {
                    self.vgs_menu = crate::vgs::Menu::Main;
                    self.set_overlay(OverlayMode::None);
                } else {
                    self.publish_ui();
                }
            }
            "cg_strafetrailradius" => {
                self.strafe_trails.settings.radius = finite_number()?.clamp(0.1, 100.0);
                self.mark_config_dirty();
            }
            "cg_strafetraillife" => {
                self.strafe_trails.settings.life_seconds = finite_number()?.clamp(0.1, 3600.0);
                self.mark_config_dirty();
            }
            "cg_strafetrailfps" => {
                self.strafe_trails.settings.fps = finite_number()?.clamp(1.0, 1000.0);
                self.mark_config_dirty();
            }
            "cg_strafetrailplums" => {
                self.strafe_trails.settings.plums = boolean()?;
                self.mark_config_dirty();
            }
            "cg_strafetrailghost" => {
                self.strafe_trails.settings.ghost = boolean()?;
                self.mark_config_dirty();
            }
            "cg_strafetrailplayers" => {
                let mask = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected an unsigned integer bitmask"))?;
                self.strafe_trails.set_players_mask(mask);
                self.mark_config_dirty();
            }
            "cg_logstrafetrail" => {
                let active_dir =
                    jka_assets::pk3::active_game_directory(&self.base, self.game.as_deref());
                self.strafe_trails.set_log_name(&active_dir, value)?;
                self.strafe_trail_log_input = self.strafe_trails.settings.log_name.clone();
                self.mark_config_dirty();
            }
            "cg_strafetraildistance" => {
                self.strafe_trails.settings.draw_distance =
                    finite_number()?.clamp(256.0, 131_072.0);
                self.mark_config_dirty();
            }
            "cg_rghostalpha" => {
                self.race_ghost_alpha = finite_number()?.clamp(0.02, 1.0);
                if let Some(session) = self.game_session.as_mut() {
                    session.race_ghost_alpha = self.race_ghost_alpha;
                }
                self.mark_config_dirty();
            }
            "cg_rghostname" => {
                self.race_ghost_name = boolean()?;
                self.mark_config_dirty();
            }
            "cg_rghosttrail" => {
                self.race_ghost_trail = boolean()?;
                self.mark_config_dirty();
            }
            "cg_rghostvelocitydelta" => {
                self.race_ghost_velocity_delta = boolean()?;
                self.mark_config_dirty();
            }
            "cg_rghostdistancedelta" => {
                self.race_ghost_distance_delta = boolean()?;
                self.mark_config_dirty();
            }
            "cg_rghostdemobaseurl" => {
                self.set_race_ghost_demo_base_url(value)?;
            }
            "cg_screenshake" => {
                self.screen_shake = finite_number()?.clamp(0.0, 2.0) as u8;
                if let Some(session) = self.game_session.as_mut() {
                    session.screen_shake_level = self.screen_shake;
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_jumpheightshade" => {
                self.jump_height_shade = boolean()?;
                self.mark_config_dirty();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_forcemodel" => {
                let parsed = ForcedPlayerModels::parse(value)?;
                self.force_model = parsed
                    .as_ref()
                    .map(ForcedPlayerModels::serialize)
                    .unwrap_or_else(|| "0".to_owned());
                self.forced_player_models = parsed;
                if let Some(session) = self.game_session.as_mut() {
                    session.forced_player_models = self.forced_player_models.clone();
                }
                self.mark_config_dirty();
                // PlayerPresenter compares the requested model/skin with each
                // entity runtime every frame, so the visual swap is immediate
                // and does not require a reconnect or a client-info rebuild.
                self.publish_snapshot();
            }
            "cg_zoomfov" => {
                self.japro_zoom_fov = finite_number()?;
                self.mark_config_dirty();
                self.publish_snapshot();
            }
            "cg_fkduration" => {
                self.japro_fk_duration = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| format!("{name}: expected an integer"))?;
                self.mark_config_dirty();
            }
            "cg_fkfirstjumpduration" => {
                self.japro_fk_first_jump_duration = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| format!("{name}: expected an integer"))?;
                self.mark_config_dirty();
            }
            "cg_fksecondjumpdelay" => {
                self.japro_fk_second_jump_delay = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| format!("{name}: expected an integer"))?;
                self.mark_config_dirty();
            }
            "sv_master1" | "sv_master2" | "sv_master3" | "sv_master4" | "sv_master5" => {
                let slot = lower
                    .strip_prefix("sv_master")
                    .and_then(|number| number.parse::<usize>().ok())
                    .and_then(|number| number.checked_sub(1))
                    .filter(|slot| *slot < server_browser::MAX_MASTER_SLOTS)
                    .ok_or_else(|| format!("{name}: invalid master slot"))?;
                let master = value.trim();
                if master.len() > 255 {
                    return Err(format!("{name}: master address is too long"));
                }
                self.server_browser.master_servers[slot] = master.to_owned();
                if !master.is_empty() {
                    self.server_browser.master_drafts[slot] = master.to_owned();
                }
                self.server_browser.status_text =
                    "Master server settings changed â€” press Refresh to query them".to_owned();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_drawcrosshair" => {
                let error = || {
                    format!(
                        "cg_drawCrosshair: expected an integer from 0 to {}",
                        ui::CROSSHAIR_STYLE_MAX
                    )
                };
                let style = value.trim().parse::<u8>().map_err(|_| error())?;
                if style > ui::CROSSHAIR_STYLE_MAX {
                    return Err(error());
                }
                self.crosshair.style = style;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_crosshairimage" => {
                let error = || {
                    format!(
                        "cg_crosshairImage: expected an integer from 0 to {}",
                        ui::CROSSHAIR_IMAGE_COUNT
                    )
                };
                let image = value.trim().parse::<u8>().map_err(|_| error())?;
                if image > ui::CROSSHAIR_IMAGE_COUNT {
                    return Err(error());
                }
                self.crosshair.image = image;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_dynamiccrosshair" => {
                let mode = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| "cg_dynamicCrosshair: expected 0, 1, or 2".to_owned())?;
                if mode > 2 {
                    return Err("cg_dynamicCrosshair: expected 0, 1, or 2".to_owned());
                }
                self.crosshair.dynamic = mode;
                self.crosshair_scan_at = Instant::now() - Duration::from_secs(1);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_crosshairidentifytarget" => {
                self.crosshair.identify_target = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_drawcrosshairnames" => {
                // > 0: seconds a name lingers; < 0: only while aimed at.
                self.crosshair.names = finite_number()?.clamp(-1000.0, 1000.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_drawcrosshairnamescolours" => {
                self.crosshair.names_colours = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_drawcrosshairnamesopacity" => {
                self.crosshair.names_opacity = finite_number()?.clamp(0.0, 1.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_drawplayernames" => {
                let mode = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "cg_drawPlayerNames: expected 0, 1, or 2".to_owned())?;
                if !(0..=2).contains(&mode) {
                    return Err("cg_drawPlayerNames: expected 0, 1, or 2".to_owned());
                }
                self.player_names.mode = mode;
                if mode == 0 {
                    self.player_name_visible = [false; 32];
                } else {
                    self.player_name_scan_at = Instant::now() - Duration::from_secs(1);
                }
                self.mark_config_dirty();
                self.publish_snapshot();
            }
            "cg_drawplayernamesscale" => {
                self.player_names.scale = finite_number()?.clamp(0.05, 4.0);
                self.mark_config_dirty();
                self.publish_snapshot();
            }
            "cg_crosshairsize" => {
                self.crosshair.size = finite_number()?.clamp(4.0, 96.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_crosshairstrength" => {
                self.crosshair.strength = finite_number()?.clamp(0.0, ui::CROSSHAIR_STRENGTH_MAX);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_crosshaircolor" => {
                let values = value
                    .split_whitespace()
                    .map(str::parse::<i32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        "cg_crosshairColor: expected R G B A values from 0 to 255".to_owned()
                    })?;
                if values.len() != 4 {
                    return Err(
                        "cg_crosshairColor: expected R G B A values from 0 to 255".to_owned()
                    );
                }
                self.crosshair.color = [
                    values[0].clamp(0, 255) as u8,
                    values[1].clamp(0, 255) as u8,
                    values[2].clamp(0, 255) as u8,
                    values[3].clamp(0, 255) as u8,
                ];
                self.mark_config_dirty();
                self.publish_ui();
            }
            hud_cvar if HudElementId::from_cvar(hud_cvar).is_some() => {
                let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                    format!("{name}: expected <anchor> <x> <y> <scale> [width height], e.g. bl 24 -58 1")
                })?;
                let id = HudElementId::from_cvar(hud_cvar).expect("guarded above");
                *self.hud_layout.element_mut(id) = layout;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_hudsnap" => {
                self.hud_layout.snap_to_grid = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_hudgridsize" => {
                self.hud_layout.grid_size = finite_number()?.clamp(1.0, 64.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_movementkeys" => {
                let mode = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| "cg_movementKeys: expected 0..4".to_owned())?;
                if mode > 4 {
                    return Err("cg_movementKeys: expected 0..4".to_owned());
                }
                self.movement_keys_hud.mode = mode;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_movementkeysx" => {
                self.movement_keys_hud.x = finite_number()?.clamp(-640.0, 640.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_movementkeysy" => {
                self.movement_keys_hud.y = finite_number()?.clamp(-480.0, 480.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_movementkeyssize" => {
                self.movement_keys_hud.size = finite_number()?.clamp(0.25, 4.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_movementkeyswalk" => {
                self.movement_keys_hud.walk = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_strafehelper" => {
                self.strafe_helper.flags = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| "cg_strafeHelper: expected a non-negative bitmask".to_owned())?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_strafehelper_fps" => {
                self.strafe_helper.fps = finite_number()?.clamp(0.0, 1000.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_strafehelperoffset" => {
                self.strafe_helper.offset = finite_number()?.clamp(-1000.0, 1000.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_strafehelperlinewidth" => {
                self.strafe_helper.line_width = finite_number()?.clamp(0.25, 5.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_strafehelperprecision" => {
                self.strafe_helper.precision = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| "cg_strafeHelperPrecision: expected 100..10000".to_owned())?
                    .clamp(100, 10000);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_strafehelpercutoff" => {
                self.strafe_helper.cutoff = finite_number()?.clamp(0.0, 480.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_strafehelperactivecolor" => {
                let values = value
                    .split_whitespace()
                    .map(str::parse::<i32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        "cg_strafeHelperActiveColor: expected R G B A values from 0 to 255"
                            .to_owned()
                    })?;
                if values.len() != 4 {
                    return Err(
                        "cg_strafeHelperActiveColor: expected R G B A values from 0 to 255"
                            .to_owned(),
                    );
                }
                self.strafe_helper.active_color = [
                    values[0].clamp(0, 255) as u8,
                    values[1].clamp(0, 255) as u8,
                    values[2].clamp(0, 255) as u8,
                    values[3].clamp(0, 255) as u8,
                ];
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_strafehelperinactivealpha" => {
                self.strafe_helper.inactive_alpha = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "cg_strafeHelperInactiveAlpha: expected 0..255".to_owned())?
                    .clamp(0, 255) as u8;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "model" => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err("model: expected model[/skin]".to_owned());
                }
                self.solo_client_info = ClientInfo::solo_model(trimmed);
                self.sync_local_client_info();
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "sensitivity" => {
                self.mouse_input.sensitivity = finite_number()?;
                if let Some(player) = &mut self.local_server {
                    player.set_mouse_input_settings(self.mouse_input);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "m_yaw" => {
                self.mouse_input.yaw = finite_number()?;
                if let Some(player) = &mut self.local_server {
                    player.set_mouse_input_settings(self.mouse_input);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "m_pitch" => {
                self.mouse_input.pitch = finite_number()?;
                if let Some(player) = &mut self.local_server {
                    player.set_mouse_input_settings(self.mouse_input);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cl_mouseaccel" => {
                self.mouse_input.accel = finite_number()?;
                if let Some(player) = &mut self.local_server {
                    player.set_mouse_input_settings(self.mouse_input);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_fov" => {
                let fov = finite_number()?;
                if !(crate::camera::MIN_CG_FOV..=crate::camera::MAX_CG_FOV).contains(&fov) {
                    return Err(format!(
                        "{name}: expected {}..{} degrees",
                        crate::camera::MIN_CG_FOV,
                        crate::camera::MAX_CG_FOV
                    ));
                }
                self.camera.set_cg_fov(fov);
                self.mark_config_dirty();
                self.publish_snapshot();
                self.publish_ui();
            }
            "s_volume" => {
                self.audio.effects_volume = finite_number()?.clamp(0.0, 1.0);
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_volumevoice" => {
                self.audio.voice_volume = finite_number()?.clamp(0.0, 1.0);
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_musicvolume" => {
                self.audio.music_volume = finite_number()?.clamp(0.0, 1.0);
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_separation" => {
                self.audio.separation = finite_number()?.clamp(0.0, 1.0);
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_jumpsounds" | "cg_rollsounds" | "cg_duelsounds" | "cg_killsounds"
            | "cg_killmessage" | "cg_drawrewards" | "cg_hitsounds" => {
                let max = match lower.as_str() {
                    "cg_killsounds" | "cg_drawrewards" => 2.0,
                    "cg_hitsounds" => 6.0,
                    _ => 3.0,
                };
                let level = finite_number()?.clamp(0.0, max) as u8;
                match lower.as_str() {
                    "cg_jumpsounds" => self.audio.game.jump = level,
                    "cg_rollsounds" => self.audio.game.roll = level,
                    "cg_duelsounds" => self.audio.game.duel = level,
                    "cg_killsounds" => self.audio.game.kill = level,
                    "cg_killmessage" => self.audio.game.kill_message = level,
                    "cg_drawrewards" => {
                        self.audio.game.draw_rewards = level;
                        if let Some(session) = &mut self.game_session {
                            session.reward_active = None;
                            session.reward_queue.clear();
                        }
                    }
                    _ => self.audio.game.hit = level,
                }
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_racesounds" => {
                self.audio.game.race_sounds = finite_number()?.clamp(0.0, 255.0) as u8;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_chatsounds" => {
                self.audio.game.chat_sounds = finite_number()?.clamp(0.0, 2.0) as u8;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_footsteps" => {
                let level = finite_number()?.clamp(0.0, 4.0) as u8;
                self.audio.game.footsteps = level;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_ghoul2marks" => {
                self.audio.game.g2_marks = finite_number()?.clamp(0.0, 64.0) as u8;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_scoreplums" => {
                self.audio.game.score_plums = boolean()?;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_autoswitch" => {
                self.audio.game.auto_switch = finite_number()?.clamp(0.0, 2.0) as u8;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_blood" => {
                self.audio.game.blood = finite_number()?.clamp(0.0, 2.0) as u8;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_ambientsounds" => {
                self.audio.game.ambient = boolean()?;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_duelmusic" => {
                self.audio.game.duel_music = boolean()?;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_notaunt" => {
                self.audio.game.no_taunt = boolean()?;
                self.apply_game_sounds();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_mutewhenunfocused" => {
                self.audio.mute_when_unfocused = boolean()?;
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_steamaudio" => {
                let enabled = boolean()?;
                self.audio.steam_audio = enabled;
                if !enabled {
                    self.steam_audio_bake = None;
                    self.steam_audio_bake_progress = None;
                    self.steam_audio_bake_error = None;
                }
                let acoustic_mesh = self.steam_audio_acoustic_mesh.clone();
                let bake = self.steam_audio_bake.clone();
                if let Some(sound) = self
                    .game_session
                    .as_mut()
                    .and_then(|session| session.sound_presenter.as_mut())
                {
                    sound.set_steam_audio_enabled(enabled);
                    sound.set_steam_audio_map(acoustic_mesh, bake);
                }
                println!(
                    "Steam Audio: {} ({})",
                    if enabled { "enabled" } else { "disabled" },
                    if enabled {
                        "acoustic geometry/cache lookup will run on the next map load; first-time bakes continue in the background"
                    } else {
                        "runtime use and future acoustic BSP/bake preparation gated off"
                    }
                );
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_steamaudiobinaural" => {
                let enabled = boolean()?;
                self.audio.steam_audio_binaural = enabled;
                if let Some(sound) = self
                    .game_session
                    .as_mut()
                    .and_then(|session| session.sound_presenter.as_mut())
                {
                    sound.set_steam_audio_binaural_enabled(enabled);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_steamaudioenvironmental" => {
                let enabled = boolean()?;
                self.audio.steam_audio_environmental = enabled;
                if let Some(sound) = self
                    .game_session
                    .as_mut()
                    .and_then(|session| session.sound_presenter.as_mut())
                {
                    sound.set_steam_audio_environmental_enabled(enabled);
                }
                println!(
                    "Steam Audio environmental acoustics: {} (live direct occlusion/transmission; no map reload or rebake required)",
                    if enabled { "enabled" } else { "disabled" },
                );
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_thirdperson" => {
                self.third_person.enabled = boolean()?;
                self.third_person_camera.reset();
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_speccamera" => {
                let mode = value.trim().parse::<i32>().map_err(|_| {
                    "cg_specCamera expects 0 (first), 1 (third), or 2 (orbit)".to_owned()
                })?;
                if !(0..=2).contains(&mode) {
                    return Err(
                        "cg_specCamera expects 0 (first), 1 (third), or 2 (orbit)".to_owned()
                    );
                }
                self.spectator_camera.mode = SpectatorCameraMode::from_i32(mode);
                self.spectator_camera_state.reset();
                self.third_person_camera.reset();
                self.sync_demo_camera_capture();
                self.mark_config_dirty();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_speccameramotion" => {
                self.spectator_camera.motion_direction = boolean()?;
                self.third_person_camera.reset();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_specorbitrange" => {
                self.spectator_camera.orbit_range = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?
                    .clamp(MIN_SPECTATOR_ORBIT_RANGE, MAX_SPECTATOR_ORBIT_RANGE);
                self.third_person_camera.reset();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_fpls" => {
                self.first_person_lightsaber = boolean()?;
                self.third_person_camera.reset();
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_sabertrail" => {
                self.saber_trail = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "cg_saberTrail expects 0, 1, or 2".to_owned())?
                    .clamp(0, 2);
                self.mark_config_dirty();
                self.push_console_line(format!("cg_saberTrail = {}", self.saber_trail));
            }
            "cg_saberteamcolors" => {
                self.saber_team_colors = boolean()?;
                if let Some(session) = self.game_session.as_mut() {
                    session
                        .player_presenter
                        .set_saber_team_colors(self.saber_team_colors);
                }
                self.mark_config_dirty();
                self.publish_snapshot();
            }
            "cg_saberstaffmulticolor" => {
                self.saber_staff_multi_color = boolean()?;
                if let Some(session) = self.game_session.as_mut() {
                    session
                        .player_presenter
                        .set_saber_staff_multi_color(self.saber_staff_multi_color);
                }
                self.mark_config_dirty();
                self.publish_snapshot();
            }
            "cg_drawteamoverlay" => {
                self.team_overlay.mode = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "cg_drawTeamOverlay expects 0..6".to_owned())?
                    .clamp(0, 6);
                // CG_UpdateCvars parity: mirror the visual cvar into the hidden
                // CVAR_USERINFO teamoverlay bit so the server sends/stops tinfo.
                let requested = self.team_overlay.mode > 0;
                if self.network.team_overlay != requested {
                    self.network.team_overlay = requested;
                    self.request_userinfo_send();
                }
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_drawteamoverlayx" => {
                self.team_overlay.x = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "cg_drawTeamOverlayX expects an integer".to_owned())?;
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_drawteamoverlayy" => {
                self.team_overlay.y = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "cg_drawTeamOverlayY expects an integer".to_owned())?;
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_drawteamoverlayweapons" => {
                self.team_overlay.weapons = boolean()?;
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_drawteamoverlayscale" => {
                self.team_overlay.scale = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| "cg_drawTeamOverlayScale expects a number".to_owned())?
                    .clamp(0.5, 2.5);
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_drawteamoverlaymaxhp" => {
                self.team_overlay.max_hp = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| "cg_drawTeamOverlayMaxHP expects a number".to_owned())?
                    .max(1.0);
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_drawteamoverlayforce" => {
                self.team_overlay.force = boolean()?;
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_scoredeaths" => {
                self.score_deaths = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "cg_scoreDeaths expects 0..3".to_owned())?
                    .clamp(0, 3);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_drawscores" => {
                self.draw_scores = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| "cg_drawScores expects 0..3".to_owned())?
                    .clamp(0, 3);
                self.mini_scores_shown = self.mini_scores_ui();
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_fxfps" => {
                let requested = value.trim().parse::<u32>().map_err(|_| {
                    format!(
                        "cg_fxFPS expects 0 (Legacy JKA) or {}..{} Hz",
                        crate::fx::FX_FPS_MIN,
                        crate::fx::FX_FPS_MAX
                    )
                })?;
                self.video.fx_fps = if requested == crate::fx::FX_FPS_LEGACY_JKA {
                    crate::fx::FX_FPS_LEGACY_JKA
                } else {
                    requested.clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX)
                };
                self.apply_fx_fps_settings();
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = if self.video.fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
                    "FX FPS: LEGACY JKA (ONE CONTINUOUS EFFECT INVOCATION PER PRESENTATION FRAME)"
                        .into()
                } else {
                    format!("FX FPS: {} HZ", self.video.fx_fps)
                };
            }
            "cg_fxfpsscope" => {
                let requested = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| "cg_fxFPSScope expects 0..1".to_owned())?;
                self.video.fx_fps_scope = requested.min(crate::fx::FX_FPS_SCOPE_FRAME_DRIVEN);
                self.apply_fx_fps_settings();
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status =
                    if self.video.fx_fps_scope == crate::fx::FX_FPS_SCOPE_FRAME_DRIVEN {
                        "FX FPS SCOPE: ALL FRAME-DRIVEN FX".into()
                    } else {
                        "FX FPS SCOPE: CONTINUOUS EFX".into()
                    };
            }
            "fx_physics" => {
                value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| "fx_physics expects 0..3".to_owned())?;
                self.video.fx_physics = config::normalize_fx_physics(value, self.video.fx_physics);
                self.apply_fx_physics();
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "FX PHYSICS: {}",
                    crate::fx::physics_label(self.video.fx_physics)
                );
            }
            "fx_lod" => {
                value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| "fx_lod expects 0..2".to_owned())?;
                self.video.fx_lod = config::normalize_fx_lod(value, self.video.fx_lod);
                self.apply_fx_lod();
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status =
                    format!("FX LOD: {}", crate::fx::lod_label(self.video.fx_lod));
            }
            "r_fxlodscale" => {
                value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| "r_fxLodScale expects a number".to_owned())?;
                self.video.fx_lod_scale =
                    config::normalize_lod_scale(value, self.video.fx_lod_scale);
                self.apply_fx_lod();
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!("FX LOD SCALE: {}", self.video.fx_lod_scale);
            }
            "r_lodscale" => {
                value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| "r_lodScale expects a number".to_owned())?;
                self.video.lod_scale = config::normalize_lod_scale(value, self.video.lod_scale);
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!("MODEL LOD SCALE: {}", self.video.lod_scale);
            }
            "fx_countscale" => {
                value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| "fx_countScale expects 0..1".to_owned())?;
                self.video.fx_count_scale =
                    config::normalize_fx_count_scale(value, self.video.fx_count_scale);
                self.apply_fx_lod();
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!("FX COUNT SCALE: {}", self.video.fx_count_scale);
            }
            "cg_thirdpersonalpha" => {
                self.third_person.alpha = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonangle" => {
                self.third_person.angle = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersoncameradamp" => {
                self.third_person.camera_damp = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonhorzoffset" => {
                self.third_person.horz_offset = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonpitchoffset" => {
                self.third_person.pitch_offset = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonrange" => {
                self.third_person.range = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonspecialcam" => {
                self.third_person.special_cam = boolean()?;
                // CVAR_NONE in TaystJK/OpenJK: runtime only, not archived.
                self.publish_ui();
            }
            "cg_thirdpersontargetdamp" => {
                self.third_person.target_damp = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonvertoffset" => {
                self.third_person.vert_offset = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "com_maxfps" => {
                let cap = config::normalize_fps_cap(value, self.video.fps_cap);
                if value.trim().parse::<u32>().is_err() {
                    return Err(format!("{name}: expected an integer FPS cap"));
                }
                self.set_fps_cap(cap);
            }
            "cg_drawfps" => {
                let mode = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| format!("{name}: expected 0, 1, or 2"))?;
                if mode > 2 {
                    return Err(format!("{name}: expected 0, 1, or 2"));
                }
                self.video.draw_fps = mode;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_drawtimer" => {
                self.video.draw_timer = boolean()?;
                self.game_timer_shown = None;
                self.mark_config_dirty();
                self.publish_transient_ui();
            }
            "cg_debugevents" => {
                let mode = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| format!("{name}: expected 0, 1, 2, or 3"))?;
                if mode > 3 {
                    return Err(format!("{name}: expected 0, 1, 2, or 3"));
                }
                self.cg_debug_events = mode;
                self.push_console_line(match mode {
                    0 => "^3cg_debugEvents:^7 OFF".to_owned(),
                    1 => "^3cg_debugEvents:^7 accepted events + current dispatch status".to_owned(),
                    2 => {
                        "^3cg_debugEvents:^7 accepted events + suppressed duplicate/zero candidates"
                            .to_owned()
                    }
                    _ => "^3cg_debugEvents:^7 verbose entity/resource diagnostics".to_owned(),
                });
            }
            "cg_asyncassets" => {
                let enabled = boolean()?;
                crate::asset_jobs::set_async_enabled(enabled);
                let workers = crate::asset_jobs::pool().map_or(0, |pool| pool.worker_count());
                self.push_console_line(if enabled {
                    format!("^3cg_asyncAssets:^7 ON ({workers} asset worker thread(s); cache misses never block the frame)")
                } else {
                    "^3cg_asyncAssets:^7 OFF (synchronous registration on the presenting thread)".to_owned()
                });
            }
            "cg_eventworkers" => {
                let enabled = boolean()?;
                self.cg_event_workers = enabled;
                let workers = if enabled {
                    event_workers::worker_count()
                } else {
                    0
                };
                self.push_console_line(if enabled {
                    format!("^3cg_eventWorkers:^7 ON ({workers} pool thread(s); single-event batches stay inline)")
                } else {
                    "^3cg_eventWorkers:^7 OFF (event preparation runs on the CGame thread)".to_owned()
                });
                self.publish_ui();
            }
            "pmove_msec" => {
                let parsed = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected an integer timestep in milliseconds"))?;
                self.set_physics_msec(parsed.clamp(1, 33));
            }
            "cl_timerresolution1ms" => {
                let enabled = boolean()?;
                self.set_timer_resolution_1ms(enabled)?;
            }
            "cl_input_latelatch" => {
                let enabled = boolean()?;
                self.set_input_latelatch(enabled);
            }
            "r_physics"
            | "r_physicsccd"
            | "r_physicssleeping"
            | "r_ragdolls"
            | "r_ragdollselfcollision"
            | "r_jigglephysics"
            | "r_clothphysics"
            | "r_clothbodycollision"
            | "r_clothwind"
            | "r_physicsprops"
            | "r_physicsdebris"
            | "r_physicsplayerpush"
            | "r_physicsweaponimpulses"
            | "r_physicsexplosionimpulses"
            | "r_physicsforceimpulses"
            | "r_physicsdebug"
            | "r_physicsstats" => {
                let enabled = boolean()?;
                match lower.as_str() {
                    "r_physics" => self.video.client_physics = enabled,
                    "r_physicsccd" => self.video.client_physics_ccd = enabled,
                    "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                    "r_ragdolls" => self.video.ragdolls = enabled,
                    "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                    "r_jigglephysics" => self.video.jiggle_physics = enabled,
                    "r_clothphysics" => self.video.cloth_physics = enabled,
                    "r_clothbodycollision" => self.video.cloth_body_collision = enabled,
                    "r_clothwind" => self.video.cloth_wind = enabled,
                    "r_physicsprops" => self.video.physics_props = enabled,
                    "r_physicsdebris" => self.video.physics_debris = enabled,
                    "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                    "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                    "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                    "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                    "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                    "r_physicsstats" => self.video.physics_stats = enabled,
                    _ => unreachable!(),
                }
                self.mark_config_dirty();
                if lower == "r_physics" {
                    self.ensure_map_physics_mesh();
                }
                self.publish_ui();
            }
            "r_jigglesolver" => {
                let normalized = value.trim().to_ascii_lowercase();
                self.video.jiggle_solver = match normalized.as_str() {
                    "0" | "kawaii" | "kawaiiphysics" => 0,
                    "1" | "jiggle" | "jigglephysics" | "naelstrof" => 1,
                    _ => return Err(format!("{name}: expected 0/KawaiiPhysics or 1/JigglePhysics")),
                };
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_jigglestrength"
            | "r_jigglebreaststrength"
            | "r_jiggleglutestrength"
            | "r_jigglestiffness"
            | "r_jiggledamping"
            | "r_jiggleglutelift"
            | "r_jigglejpstiffness"
            | "r_jigglejpdrag"
            | "r_jigglejpairdrag"
            | "r_jigglejpstretch"
            | "r_jigglejpsoften"
            | "r_jigglejpgravity" => {
                let amount = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a finite number"))?;
                if !amount.is_finite() {
                    return Err(format!("{name}: expected a finite number"));
                }
                match lower.as_str() {
                    "r_jigglestrength" => self.video.jiggle_strength = amount.clamp(0.0, 2.0),
                    "r_jigglebreaststrength" => {
                        self.video.jiggle_breast_strength = amount.clamp(0.0, 2.0)
                    }
                    "r_jiggleglutestrength" => {
                        self.video.jiggle_glute_strength = amount.clamp(0.0, 2.0)
                    }
                    "r_jigglestiffness" => self.video.jiggle_stiffness = amount.clamp(0.0, 3.0),
                    "r_jiggledamping" => self.video.jiggle_damping = amount.clamp(0.0, 3.0),
                    "r_jiggleglutelift" => self.video.jiggle_glute_lift = amount.clamp(-0.4, 0.6),
                    "r_jigglejpstiffness" => self.video.jiggle_jp_stiffness = amount.clamp(0.0, 1.0),
                    "r_jigglejpdrag" => self.video.jiggle_jp_drag = amount.clamp(0.0, 1.0),
                    "r_jigglejpairdrag" => self.video.jiggle_jp_air_drag = amount.clamp(0.0, 1.0),
                    "r_jigglejpstretch" => self.video.jiggle_jp_stretch = amount.clamp(0.0, 1.0),
                    "r_jigglejpsoften" => self.video.jiggle_jp_soften = amount.clamp(0.0, 1.0),
                    "r_jigglejpgravity" => self.video.jiggle_jp_gravity = amount.clamp(0.0, 2.0),
                    _ => unreachable!(),
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_clothbodyclearance"
            | "r_clothairresistance"
            | "r_clothturnresponse"
            | "r_clothanimationinfluence" => {
                let amount = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a finite number"))?;
                if !amount.is_finite() {
                    return Err(format!("{name}: expected a finite number"));
                }
                match lower.as_str() {
                    "r_clothbodyclearance" => {
                        self.video.cloth_body_clearance = amount.clamp(0.0, 4.0)
                    }
                    "r_clothairresistance" => {
                        self.video.cloth_air_resistance = amount.clamp(0.0, 4.0)
                    }
                    "r_clothturnresponse" => {
                        self.video.cloth_turn_response = amount.clamp(0.0, 4.0)
                    }
                    "r_clothanimationinfluence" => {
                        self.video.cloth_animation_influence = amount.clamp(0.0, 1.0)
                    }
                    _ => unreachable!(),
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_physicshz" => {
                let requested = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 30, 60, 120, or 240"))?;
                if ![30_u32, 60, 120, 240].contains(&requested) {
                    return Err(format!("{name}: expected 30, 60, 120, or 240"));
                }
                self.video.client_physics_hz = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_physicsmaxsubsteps" => {
                let requested = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 1, 2, 4, or 8"))?;
                if ![1_u32, 2, 4, 8].contains(&requested) {
                    return Err(format!("{name}: expected 1, 2, 4, or 8"));
                }
                self.video.client_physics_max_substeps = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ragdollmax" => {
                let requested = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 2, 4, 8, 16, or 32"))?;
                if ![2_u32, 4, 8, 16, 32].contains(&requested) {
                    return Err(format!("{name}: expected 2, 4, 8, 16, or 32"));
                }
                self.video.ragdoll_max = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ragdolllifetime" => {
                // Written as "20.0" by the config, so accept whole-valued floats.
                let requested = finite_number()
                    .ok()
                    .filter(|seconds| seconds.fract() == 0.0)
                    .map(|seconds| seconds as u32)
                    .ok_or_else(|| format!("{name}: expected 5, 10, 20, 30, or 60"))?;
                if ![5_u32, 10, 20, 30, 60].contains(&requested) {
                    return Err(format!("{name}: expected 5, 10, 20, 30, or 60"));
                }
                self.video.ragdoll_lifetime = requested as f32;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_dismember" => {
                let requested = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| "cg_dismember expects 0, 1, or 2".to_owned())?;
                self.video.dismemberment = requested.min(2);
                self.physics_menu_changed();
                return Ok(());
            }
            "r_dismembermax" => {
                let requested = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| "r_dismemberMax expects an integer".to_owned())?;
                self.video.dismember_max = requested.clamp(1, 128);
                self.physics_menu_changed();
                return Ok(());
            }
            "r_dismemberlifetime" => {
                let requested = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| "r_dismemberLifetime expects seconds".to_owned())?;
                if !requested.is_finite() {
                    return Err("r_dismemberLifetime expects a finite number".to_owned());
                }
                self.video.dismember_lifetime = requested.clamp(1.0, 300.0);
                self.physics_menu_changed();
                return Ok(());
            }
            "r_physicspropmax" => {
                let requested = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 32, 64, 96, 192, or 384"))?;
                if ![32_u32, 64, 96, 192, 384].contains(&requested) {
                    return Err(format!("{name}: expected 32, 64, 96, 192, or 384"));
                }
                self.video.physics_prop_max = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_physicsdebrismax" => {
                let requested = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 64, 128, 192, 384, or 768"))?;
                if ![64_u32, 128, 192, 384, 768].contains(&requested) {
                    return Err(format!("{name}: expected 64, 128, 192, 384, or 768"));
                }
                self.video.physics_debris_max = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_physicsdebrislifetime" => {
                let requested = finite_number()
                    .ok()
                    .filter(|seconds| seconds.fract() == 0.0)
                    .map(|seconds| seconds as u32)
                    .ok_or_else(|| format!("{name}: expected 2, 5, 10, 20, or 30"))?;
                if ![2_u32, 5, 10, 20, 30].contains(&requested) {
                    return Err(format!("{name}: expected 2, 5, 10, 20, or 30"));
                }
                self.video.physics_debris_lifetime = requested as f32;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_fullscreen" => {
                let mode = FullscreenMode::from_config(value).ok_or_else(|| {
                    format!("{name}: expected 0 (windowed), 1 (borderless), or 2 (exclusive)")
                })?;
                if mode != self.video.fullscreen {
                    self.set_fullscreen_mode(mode);
                }
            }
            "r_backend" => {
                self.video.renderer_backend = RendererBackend::from_config(value)
                    .ok_or_else(|| format!("{name}: expected vulkan or dx12"))?;
                self.mark_config_dirty();
                self.console_status = format!(
                    "RENDER BACKEND: {} (RUN VID_RESTART TO APPLY)",
                    self.video.renderer_backend.label()
                );
                self.publish_ui();
            }
            "r_swapinterval" => {
                let previous_vsync = self.video.vsync;
                self.video.vsync = VsyncMode::from_config(value).ok_or_else(|| {
                    format!("{name}: expected 0/off, 1/on, 2/fast, or 3/adaptive")
                })?;
                self.render_command(RenderCommand::SetVsync(self.video.vsync));
                self.mark_config_dirty();
                if self.video.vsync != previous_vsync {
                    // Entering FAST on DX12 can make the standing cap unreachable.
                    self.set_fps_cap(self.video.fps_cap);
                }
                self.publish_ui();
            }
            "r_maxframelatency" => {
                let latency = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 1, 2, or 3"))?;
                if !(1..=3).contains(&latency) {
                    return Err(format!("{name}: expected 1, 2, or 3"));
                }
                self.set_max_frame_latency(latency);
            }
            "r_ext_multisample" => {
                let samples = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected an integer sample count"))?;
                let samples = if samples <= 1 { 1 } else { samples };
                if !self.supported_msaa.contains(&samples) {
                    return Err(format!(
                        "{name}: unsupported sample count {samples}; supported {:?}",
                        self.supported_msaa
                    ));
                }
                self.video.msaa_samples = samples;
                if samples > 1 {
                    self.video.fxaa = false;
                    self.video.smaa = false;
                    self.video.taa = false;
                }
                self.render_command(RenderCommand::SetMsaa(samples));
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_texturemode" => {
                self.video.texture_filter = match value.trim().to_ascii_uppercase().as_str() {
                    "GL_NEAREST" | "GL_NEAREST_MIPMAP_NEAREST" | "GL_NEAREST_MIPMAP_LINEAR" => {
                        TextureFilter::Nearest
                    }
                    "GL_LINEAR" | "GL_LINEAR_MIPMAP_NEAREST" => TextureFilter::Bilinear,
                    "GL_LINEAR_MIPMAP_LINEAR" => TextureFilter::Trilinear,
                    _ => return Err(format!("{name}: unknown texture mode")),
                };
                self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_picmip" => {
                let picmip = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected an integer from 0 to 16"))?
                    .min(16);
                self.video.picmip = picmip;
                self.render_command(RenderCommand::SetPicmip(picmip));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_detailtextures" => {
                self.video.detail_textures =
                    DetailTextureMode::from_config(value).ok_or_else(|| {
                        format!("{name}: expected off|neutral2x|linear2x|dstcolor_one|multiply")
                    })?;
                self.render_command(RenderCommand::SetDetailTextures(self.video.detail_textures));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_detailtexture" => {
                return Err(format!("{name}: detail texture selection is fixed to AUTO"));
            }
            "r_detailtexturefade" => {
                self.video.detail_texture_fade = match value.trim().to_ascii_lowercase().as_str() {
                    "1" | "on" | "true" | "yes" => true,
                    "0" | "off" | "false" | "no" => false,
                    _ => return Err(format!("{name}: expected 0 or 1")),
                };
                self.render_command(RenderCommand::SetDetailTextureFade {
                    enabled: self.video.detail_texture_fade,
                    distance: self.video.detail_texture_fade_distance,
                });
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_detailtexturefadedistance" => {
                let distance = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 64..8192"))?;
                if !distance.is_finite() {
                    return Err(format!("{name}: expected 64..8192"));
                }
                self.video.detail_texture_fade_distance = distance.clamp(64.0, 8192.0);
                self.render_command(RenderCommand::SetDetailTextureFade {
                    enabled: self.video.detail_texture_fade,
                    distance: self.video.detail_texture_fade_distance,
                });
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ext_texture_filter_anisotropic" => {
                let level = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 0, 2, 4, 8, or 16"))?;
                self.video.texture_filter = match level {
                    0 | 1 => TextureFilter::Trilinear,
                    2 => TextureFilter::Anisotropic2x,
                    4 => TextureFilter::Anisotropic4x,
                    8 => TextureFilter::Anisotropic8x,
                    16 => TextureFilter::Anisotropic16x,
                    _ => return Err(format!("{name}: expected 0, 2, 4, 8, or 16")),
                };
                self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_customwidth" | "r_customheight" => {
                let parsed = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected an integer pixel size"))?;
                if lower == "r_customwidth" {
                    self.video.resolution[0] = parsed.max(320);
                } else {
                    self.video.resolution[1] = parsed.max(240);
                }
                self.mark_config_dirty();
                self.console_status = format!(
                    "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                    self.video.resolution[0], self.video.resolution[1]
                );
                self.publish_ui();
            }
            "r_windowx" | "r_windowy" => {
                if value.eq_ignore_ascii_case("auto") {
                    self.video.window_position = None;
                } else {
                    let parsed = value.trim().parse::<i32>().map_err(|_| {
                        format!("{name}: expected a desktop pixel coordinate or auto")
                    })?;
                    let mut position = self.video.window_position.unwrap_or([0, 0]);
                    if lower == "r_windowx" {
                        position[0] = parsed;
                    } else {
                        position[1] = parsed;
                    }
                    self.video.window_position = Some(position);
                    if self.applied_fullscreen.is_windowed() {
                        if let Some(window) = &self.window {
                            window.set_outer_position(PhysicalPosition::new(
                                position[0],
                                position[1],
                            ));
                        }
                    }
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_windowmaximized" => {
                self.video.window_maximized = boolean()?;
                if self.applied_fullscreen.is_windowed() {
                    if let Some(window) = &self.window {
                        window.set_maximized(self.video.window_maximized);
                    }
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_gamma" => self.set_gamma(finite_number()?),
            "r_gammamethod" => {
                let method = crate::gamma::GammaMethod::parse(value)
                    .ok_or_else(|| format!("{name}: expected shader, baked, hardware (or 0, 1, 2)"))?;
                self.set_gamma_method(method);
            }
            "r_modelbrightness" => {
                let value = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.set_model_brightness(value);
            }
            "r_modelbrightnesslock" => {
                let locked = boolean()?;
                self.set_model_brightness_locked(locked);
            }
            "r_dynamiclightbrightness" => {
                let value = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.set_dynamic_light_brightness(value);
            }
            "r_dynamiclightbrightnesslock" => {
                let locked = boolean()?;
                self.set_dynamic_light_brightness_locked(locked);
            }
            "r_drawtriggers" => {
                self.video.draw_triggers = boolean()?;
                self.sync_debug_volumes();
                self.console_status = format!(
                    "DRAW TRIGGERS: {}",
                    if self.video.draw_triggers {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r_drawclipbrushes" => {
                self.video.draw_clip_brushes = boolean()?;
                self.sync_debug_volumes();
                self.console_status = format!(
                    "DRAW CLIP BRUSHES: {}",
                    if self.video.draw_clip_brushes {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r_drawentities" => {
                self.set_draw_entities(boolean()?);
                self.console_status = format!(
                    "DRAW ENTITIES: {}",
                    if self.video.draw_entities {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r_drawmapmodels" => {
                self.video.draw_map_models = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "MAP MODELS: {}",
                    if self.video.draw_map_models {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r_showtris" => {
                let mask = value.trim().parse::<u32>().map_err(|_| {
                    format!(
                        "{name}: expected an integer bitmask (0..{})",
                        ui::wireframe::ALL
                    )
                })?;
                if mask & !ui::wireframe::ALL != 0 {
                    return Err(format!(
                        "{name}: unsupported wireframe bits; valid range is 0..{}",
                        ui::wireframe::ALL
                    ));
                }
                if mask != 0 && !self.wireframe_supported {
                    return Err("r_showtris: wireframe overlay is not supported by this GPU".into());
                }
                self.video.wireframe_mask = mask;
                self.render_command(RenderCommand::SetWireframeMask(mask));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_skipui" => {
                self.video.skip_ui = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "developer" => {
                let level = match Self::parse_console_bool(value) {
                    Some(enabled) => u8::from(enabled),
                    None => value
                        .trim()
                        .parse::<u8>()
                        .map_err(|_| format!("{name}: expected 0..3 (off/on are also accepted)"))?,
                };
                if level > 3 {
                    return Err(format!("{name}: expected 0..3"));
                }
                self.video.developer_level = level;
                self.video.developer_tools = level != 0;
                crate::logging::set_developer_level(level);
                if !self.video.developer_tools {
                    self.forget_trace();
                    self.render_command(RenderCommand::ClearSurfaceInspection);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_verbose" => {
                let level = match Self::parse_console_bool(value) {
                    Some(enabled) => u8::from(enabled),
                    None => value
                        .trim()
                        .parse::<u8>()
                        .map_err(|_| format!("{name}: expected 0..3 (off/on are also accepted)"))?,
                };
                if level > 3 {
                    return Err(format!("{name}: expected 0..3"));
                }
                self.video.renderer_verbose = level;
                crate::logging::set_renderer_verbose_level(level);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_novis" => {
                self.video.pvs_mode = if boolean()? {
                    PvsMode::Off
                } else {
                    PvsMode::Auto
                };
                self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_pvsmode" => {
                self.video.pvs_mode = match value.trim().to_ascii_lowercase().as_str() {
                    "off" | "0" => PvsMode::Off,
                    "minimal" | "1" => PvsMode::Minimal,
                    "full" | "2" => PvsMode::Full,
                    "auto" | "3" | "auto2" | "4" | "auto3" | "5" | "auto4" | "batched"
                    | "pvsbatched" | "6" => PvsMode::Auto,
                    _ => return Err(format!("{name}: expected off|minimal|full|auto")),
                };
                self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_grass" => {
                self.video.grass = boolean()?;
                self.mark_config_dirty();
                self.console_status = format!(
                    "PROCEDURAL GRASS: {} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                    if self.video.grass { "ON" } else { "OFF" }
                );
                self.publish_ui();
            }
            "r_clouds" => {
                self.video.clouds = boolean()?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_cloudquality" => {
                self.set_cloud_quality(finite_number()?);
            }
            "r_weatherwind" => {
                let values = value
                    .split_whitespace()
                    .map(str::parse::<f32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        format!("{name}: expected speed direction gust direction_variation")
                    })?;
                if values.len() != 4 || !values.iter().all(|v| v.is_finite()) {
                    return Err(format!("{name}: expected four finite numbers"));
                }
                self.set_weather_wind(crate::ocean::OceanWind {
                    speed: values[0],
                    direction: values[1],
                    gust: values[2],
                    shift: values[3],
                });
            }
            "r_ocean" => {
                self.video.ocean = boolean()?;
                // OFF releases the FFT resources immediately, and ON can be
                // restored live only when the loaded world already carries its
                // promoted ocean clipmap.
                if !self.video.ocean || self.applied_ocean {
                    self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_oceanmapsize" => {
                let requested = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 128|256|512|1024"))?;
                if !matches!(requested, 128 | 256 | 512 | 1024) {
                    return Err(format!("{name}: expected 128|256|512|1024"));
                }
                self.video.ocean_settings.map_size = requested;
                self.commit_ocean_settings();
            }
            "r_oceanmeshquality" => {
                self.video.ocean_settings.mesh_quality =
                    match value.trim().to_ascii_lowercase().as_str() {
                        "0" | "low" => 0,
                        "1" | "high" => 1,
                        _ => return Err(format!("{name}: expected 0|1 or low|high")),
                    };
                self.commit_ocean_settings();
            }
            "r_oceanupdates" => {
                self.video.ocean_settings.updates_per_second = finite_number()?.clamp(0.0, 60.0);
                self.commit_ocean_settings();
            }
            "r_oceanseaspray" => {
                self.video.ocean_settings.sea_spray = boolean()?;
                self.commit_ocean_settings();
            }
            "r_oceanwindfoam" => {
                self.video.ocean_settings.wind_foam_streaks = boolean()?;
                self.commit_ocean_settings();
            }
            "r_oceanfogcolor" => {
                let values = value
                    .split_whitespace()
                    .map(str::parse::<f32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "Expected three finite color components".to_owned())?;
                if values.len() != 3 || !values.iter().all(|v| v.is_finite()) {
                    return Err("Expected three finite color components".to_owned());
                }
                self.video.ocean_settings.optics.fog_color = [values[0], values[1], values[2]];
                self.commit_ocean_settings();
            }
            "r_oceanfogdistance"
            | "r_oceantransparency"
            | "r_oceandepthdarkening"
            | "r_oceanrefraction"
            | "r_oceancaustics"
            | "r_oceanunderwatercull" => {
                let v = value
                    .parse::<f32>()
                    .map_err(|_| "Expected a finite number".to_owned())?;
                if !v.is_finite() {
                    return Err("Expected a finite number".to_owned());
                }
                let o = &mut self.video.ocean_settings.optics;
                match name.to_ascii_lowercase().as_str() {
                    "r_oceanfogdistance" => o.fog_distance = v,
                    "r_oceantransparency" => o.transparency = v,
                    "r_oceandepthdarkening" => o.depth_darkening = v,
                    "r_oceanrefraction" => o.refraction = v,
                    "r_oceancaustics" => o.caustics = v,
                    _ => o.underwater_cull = v,
                }
                self.commit_ocean_settings();
            }
            // The resolution is always the custom one (r_customWidth/Height);
            // the config writes -1 to say so.
            "r_mode" => {
                let mode = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| format!("{name}: expected an integer"))?;
                if mode != -1 {
                    return Err(format!(
                        "{name}: only -1 (custom, r_customWidth/r_customHeight) is supported"
                    ));
                }
            }
            "r_cloudtype" => {
                self.video.cloud_type = ui::CloudType::from_config(value)
                    .ok_or_else(|| format!("{name}: expected cumulus|stratus|storm"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_cloudcoverage" => self.set_cloud_coverage(finite_number()?),
            "r_cloudheight" => self.set_cloud_height(finite_number()?),
            "r_cloudthickness" => self.set_cloud_thickness(finite_number()?),
            "r_cloudshear"
            | "r_cloudbasevariation"
            | "r_cloudaerial"
            | "r_cloudhistoryblend"
            | "r_cloudmotionreject"
            | "r_cloudthicknessvariation"
            | "r_cloudsize" => {
                let amount = finite_number()?;
                let video = &mut self.video;
                match lower.as_str() {
                    "r_cloudshear" => video.cloud_shear = amount.clamp(0.0, 1.0),
                    "r_cloudbasevariation" => video.cloud_base_variation = amount.clamp(0.0, 1.0),
                    "r_cloudaerial" => video.cloud_aerial = amount.clamp(0.0, 1.0),
                    "r_cloudhistoryblend" => video.cloud_history_blend = amount.clamp(0.0, 0.98),
                    "r_cloudmotionreject" => video.cloud_motion_reject = amount.clamp(0.0, 1.0),
                    "r_cloudthicknessvariation" => {
                        video.cloud_thickness_variation = amount.clamp(0.0, 1.0)
                    }
                    _ => video.cloud_size = amount.clamp(0.0, 1.0),
                }
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_cloudskyambient" | "r_cloudhistorydepthreject" => {
                let enabled = boolean()?;
                if lower == "r_cloudskyambient" {
                    self.video.cloud_sky_ambient = enabled;
                } else {
                    self.video.cloud_history_depth_reject = enabled;
                }
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_oceanroughness" => {
                self.video.ocean_settings.roughness = finite_number()?;
                self.commit_ocean_settings();
            }
            "r_oceannormalstrength" => {
                self.video.ocean_settings.normal_strength = finite_number()?;
                self.commit_ocean_settings();
            }
            "r_oceanwatercolor" => {
                self.video.ocean_settings.water_color = Self::parse_cvar_floats(name, value)?;
                self.commit_ocean_settings();
            }
            "r_oceanfoamcolor" => {
                self.video.ocean_settings.foam_color = Self::parse_cvar_floats(name, value)?;
                self.commit_ocean_settings();
            }
            "r_oceancascade1" | "r_oceancascade2" | "r_oceancascade3" => {
                let v: [f32; 12] = Self::parse_cvar_floats(name, value)?;
                let index = usize::from(lower.as_bytes()[lower.len() - 1] - b'1');
                self.video.ocean_settings.cascades[index] = crate::ocean::CascadeSettings {
                    tile_length: [v[0], v[1]],
                    displacement_scale: v[2],
                    normal_scale: v[3],
                    wind_speed: v[4],
                    wind_direction: v[5],
                    fetch_length: v[6],
                    swell: v[7],
                    spread: v[8],
                    detail: v[9],
                    whitecap: v[10],
                    foam_amount: v[11],
                };
                self.commit_ocean_settings();
            }
            "r_oceanauthoring" => {
                let words: Vec<&str> = value.split_whitespace().collect();
                let expected = || format!("{name}: expected ten numbers and a seed");
                if words.len() != 11 {
                    return Err(expected());
                }
                let mut f = [0.0_f32; 10];
                for (slot, word) in f.iter_mut().zip(&words) {
                    *slot = word
                        .parse::<f32>()
                        .ok()
                        .filter(|number| number.is_finite())
                        .ok_or_else(expected)?;
                }
                let seed = words[10].parse().map_err(|_| expected())?;
                self.video.ocean_settings.authored = crate::ocean::OceanAuthoring {
                    amplitude: f[0],
                    wavelength: f[1],
                    speed: f[2],
                    direction: f[3],
                    steepness: f[4],
                    slosh: f[5],
                    wind_chop: f[6],
                    foam: f[7],
                    foam_lifetime: f[8],
                    spray: f[9],
                    seed,
                }
                .sanitize();
                self.commit_ocean_settings();
            }
            "r_perftrace" => {
                self.video.perf_trace = boolean()?;
                self.render_command(RenderCommand::SetPerfTrace(self.video.perf_trace));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_pom" => {
                // Session-only (not config-dirty): parallax occlusion with r_pbr.
                self.video.pom = boolean()?;
                self.render_command(RenderCommand::SetPom(self.video.pom));
                self.publish_ui();
            }
            "r_worldpath" => {
                self.video.force_unified_world = match value.trim().to_ascii_lowercase().as_str() {
                    "auto" => false,
                    "unified" => true,
                    _ => return Err(format!("{name}: expected auto|unified")),
                };
                self.render_command(RenderCommand::SetForceUnifiedWorld(
                    self.video.force_unified_world,
                ));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_gputimings" => {
                self.video.gpu_timings = boolean()?;
                self.render_command(RenderCommand::SetGpuTimings(self.video.gpu_timings));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ghoul2skinning" => {
                self.video.ghoul2_skinning = Ghoul2SkinningMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected cpu|workers|gpu"))?;
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status =
                    format!("GHOUL2 SKINNING: {}", self.video.ghoul2_skinning.label());
            }
            "r_fxgeometry" => {
                self.video.fx_geometry = FxGeometryMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected cpu|workers|gpu"))?;
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!("FX GEOMETRY: {}", self.video.fx_geometry.label());
            }
            "r_fxzeroalphadiscard" => {
                self.video.fx_zero_alpha_discard = boolean()?;
                self.render_command(RenderCommand::SetFxZeroAlphaDiscard(
                    self.video.fx_zero_alpha_discard,
                ));
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "FX ZERO-ALPHA DISCARD: {}",
                    if self.video.fx_zero_alpha_discard {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r_ghoul2earlycull" => {
                self.video.ghoul2_early_cull = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "GHOUL2 EARLY CULL: {}",
                    if self.video.ghoul2_early_cull {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r_lodbias" => {
                let bias = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| format!("{name}: expected a non-negative integer"))?;
                self.video.ghoul2_lod_bias = bias.max(0);
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!("MODEL LOD BIAS: {}", self.video.ghoul2_lod_bias);
            }
            "r_ghoul2batchdraws" => {
                self.video.ghoul2_batch_draws = Ghoul2BatchMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected 0|1|2 or off|adaptive|force"))?;
                self.render_command(RenderCommand::SetGhoul2BatchDraws(
                    self.video.ghoul2_batch_draws,
                ));
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "GHOUL2 BATCH DRAWS: {}",
                    self.video.ghoul2_batch_draws.label()
                );
            }
            "r_ghoul2animsmooth" => {
                let factor = value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("{name}: expected a number"))?;
                // jaPRO only activates smoothing strictly inside (0, 1); 1.0+
                // is a no-op there, not "maximum" smoothing, so don't clamp
                // the upper bound here either.
                self.video.ghoul2_anim_smooth = factor.max(0.0);
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status =
                    format!("GHOUL2 ANIM SMOOTH: {}", self.video.ghoul2_anim_smooth);
            }
            "r_grassprecompute" => {
                self.video.grass_precompute = boolean()?;
                self.render_command(RenderCommand::SetGrassPrecompute(
                    self.video.grass_precompute,
                ));
                println!(
                    "Grass A/B: precompute={}",
                    u8::from(self.video.grass_precompute)
                );
            }
            "r_grassmidlod" => {
                self.video.grass_mid_lod = boolean()?;
                self.render_command(RenderCommand::SetGrassMidLod(self.video.grass_mid_lod));
                println!("Grass A/B: mid_lod={}", u8::from(self.video.grass_mid_lod));
            }
            "r_grassfronttoback" => {
                self.video.grass_front_to_back = boolean()?;
                self.render_command(RenderCommand::SetGrassFrontToBack(
                    self.video.grass_front_to_back,
                ));
                println!(
                    "Grass A/B: front_to_back={}",
                    u8::from(self.video.grass_front_to_back)
                );
            }
            "r_contactshadowdebug" => {
                self.video.contact_shadow_debug = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| "r_contactShadowDebug takes 0 or 1".to_owned())?
                    .min(1);
                self.render_command(RenderCommand::SetContactShadowDebug(
                    self.video.contact_shadow_debug,
                ));
                println!("Contact shadow debug={}", self.video.contact_shadow_debug);
            }
            "r_staticbspaomode" => {
                self.video.static_bsp_ao_lightmap = match value.to_ascii_lowercase().as_str() {
                    "lightmap" | "lightmap-space" | "1" => true,
                    "vertex" | "per-vertex" | "0" => false,
                    _ => return Err("usage: r_staticBspAoMode <vertex|lightmap>".into()),
                };
                if self.video.static_bsp_ao {
                    self.video.ssao = false;
                }
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaosamples" => {
                let requested = value
                    .parse::<u32>()
                    .map_err(|_| "usage: r_staticBspAoSamples <8|16|32|64|128>".to_string())?;
                const VALUES: [u32; 5] = [8, 16, 32, 64, 128];
                let Some(&samples) = VALUES.iter().find(|&&samples| samples == requested) else {
                    return Err("usage: r_staticBspAoSamples <8|16|32|64|128>".into());
                };
                self.video.static_bsp_ao_samples = samples;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaoresolution" => {
                let requested = value
                    .parse::<u32>()
                    .map_err(|_| "usage: r_staticBspAoResolution <1|3|5>".to_string())?;
                let Some(&scale) = [1_u32, 3, 5].iter().find(|&&scale| scale == requested) else {
                    return Err("usage: r_staticBspAoResolution <1|3|5>".into());
                };
                self.video.static_bsp_ao_resolution = scale;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaostrength" => {
                let requested = value
                    .parse::<u32>()
                    .map_err(|_| "usage: r_staticBspAoStrength <25|50|75|100>".to_string())?;
                let Some(&strength) = [25_u32, 50, 75, 100].iter().find(|&&v| v == requested)
                else {
                    return Err("usage: r_staticBspAoStrength <25|50|75|100>".into());
                };
                self.video.static_bsp_ao_strength = strength;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaorange" => {
                let requested = value
                    .parse::<u32>()
                    .map_err(|_| "usage: r_staticBspAoRange <50|100|150|200>".to_string())?;
                let Some(&range) = [50_u32, 100, 150, 200].iter().find(|&&v| v == requested) else {
                    return Err("usage: r_staticBspAoRange <50|100|150|200>".into());
                };
                self.video.static_bsp_ao_range = range;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaocurrentcell" => {
                self.video.static_bsp_ao_current_cell = boolean()?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_hdr"
            | "r_tonemap"
            | "r_autoexposure"
            | "r_bloom"
            | "r_halation"
            | "r_ssao"
            | "r_staticbspao"
            | "r_fxaa"
            | "r_smaa"
            | "r_taa"
            | "r_contactshadows"
            | "r_cloudshadows"
            | "r_cloudtemporal"
            | "r_cloudtemporaldepthfix"
            | "r_cloudshapeevolution"
            | "r_cloudterraininteraction"
            | "r_cloudemptyskip"
            | "r_rain"
            | "r_sunoverride"
            | "r_entitysunlighting"
            | "r_reflectiondebug"
            | "r_vignette" => {
                let enabled = boolean()?;
                match lower.as_str() {
                    "r_hdr" => self.video.hdr = enabled,
                    "r_tonemap" => self.video.tone_mapping = enabled,
                    "r_autoexposure" => self.video.auto_exposure = enabled,
                    "r_bloom" => self.video.bloom = enabled,
                    "r_halation" => self.video.halation = enabled,
                    "r_ssao" => {
                        self.video.ssao = enabled;
                        if enabled {
                            self.video.static_bsp_ao = false;
                        }
                    }
                    "r_staticbspao" => {
                        self.video.static_bsp_ao = enabled;
                        if enabled {
                            self.video.ssao = false;
                        }
                    }
                    "r_fxaa" => {
                        self.video.fxaa = enabled;
                        if enabled {
                            self.video.smaa = false;
                            self.video.taa = false;
                            self.video.msaa_samples = 1;
                            self.render_command(RenderCommand::SetMsaa(1));
                        }
                    }
                    "r_smaa" => {
                        self.video.smaa = enabled;
                        if enabled {
                            self.video.fxaa = false;
                            self.video.taa = false;
                            self.video.msaa_samples = 1;
                            self.render_command(RenderCommand::SetMsaa(1));
                        }
                    }
                    "r_taa" => {
                        self.video.taa = enabled;
                        if enabled {
                            self.video.fxaa = false;
                            self.video.smaa = false;
                            self.video.msaa_samples = 1;
                            self.render_command(RenderCommand::SetMsaa(1));
                        }
                    }
                    "r_contactshadows" => self.video.contact_shadows = enabled,
                    "r_cloudshadows" => self.video.cloud_shadows = enabled,
                    "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                    "r_cloudtemporaldepthfix" => self.video.cloud_temporal_depth_fix = enabled,
                    "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                    "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                    "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                    "r_rain" => self.video.rain = enabled,
                    "r_sunoverride" => {
                        self.set_sun_override(enabled, enabled);
                        return Ok(());
                    }
                    "r_entitysunlighting" => self.video.entity_sun_lighting = enabled,
                    "r_reflectiondebug" => self.video.reflection_debug = enabled,
                    "r_vignette" => self.video.vignette = enabled,
                    _ => unreachable!(),
                }
                self.sync_post_effects();
                if lower != "r_reflectiondebug" {
                    self.mark_config_dirty();
                }
                // r_floatLightmap is only effective while HDR is active, matching
                // Rend2. If that changes the prepared lightmap representation,
                // video_restart_required() exposes the normal renderer-restart path.
                self.publish_ui();
            }
            "r_reflectionquality" => {
                self.video.reflection_quality = ReflectionQuality::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|legacy|low|medium|high|ultra"))?;
                self.reflection_quality_changed();
                self.publish_ui();
                return Ok(());
            }
            "r_sunvisibility" => {
                self.video.sun_visibility = SunVisibilityMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected legacy|sky|filtered"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_cloudrenderresolution" => {
                self.video.cloud_render_resolution = CloudRenderResolution::from_config(value)
                    .ok_or_else(|| format!("{name}: expected full|half|quarter"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_rainintensity" => {
                self.video.rain_intensity = RainIntensity::from_config(value)
                    .ok_or_else(|| format!("{name}: expected light|rain|heavy"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_puddlequality" => {
                self.video.puddle_quality = PuddleQuality::from_config(value)
                    .ok_or_else(|| format!("{name}: expected standard|high"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_puddlescatter" => {
                let amount = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_puddle_scatter(amount);
            }
            "r_raingrade" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_rain_grade(strength);
            }
            "r_footprints" => {
                self.video.footprints = FootprintMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|2d|3d"))?;
                self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                self.footprints_chosen();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_chromaticaberration" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a non-negative number"))?;
                self.set_chromatic_aberration_strength(strength);
            }
            "r_drawfog" => {
                self.video.fog_mode = FogMode::from_drawfog(value)
                    .ok_or_else(|| format!("{name}: expected 0|1|2|3"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_sunyaw" => {
                let yaw = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected degrees"))?;
                if !yaw.is_finite() {
                    return Err(format!("{name}: expected finite degrees"));
                }
                self.set_sun_yaw(yaw);
            }
            "r_sunpitch" => {
                let pitch = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected degrees"))?;
                if !pitch.is_finite() {
                    return Err(format!("{name}: expected finite degrees"));
                }
                self.set_sun_pitch(pitch);
            }
            "r_sunintensity" => {
                let intensity = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a non-negative number"))?;
                if !intensity.is_finite() || intensity < 0.0 {
                    return Err(format!("{name}: expected a non-negative finite number"));
                }
                self.set_sun_intensity(intensity);
            }
            "r_suncolor" => {
                let values = value
                    .split([',', ' '])
                    .filter(|part| !part.is_empty())
                    .map(str::parse::<f32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| format!("{name}: expected R G B in the 0..1 range"))?;
                if values.len() != 3 || values.iter().any(|v| !v.is_finite()) {
                    return Err(format!("{name}: expected R G B in the 0..1 range"));
                }
                self.set_sun_color([
                    values[0].clamp(0.0, 1.0),
                    values[1].clamp(0.0, 1.0),
                    values[2].clamp(0.0, 1.0),
                ]);
            }
            "r_fogstrength" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.set_fog_strength(strength);
            }
            "r_distancecullscale" => {
                let scale = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.set_distance_cull_scale(scale);
            }
            "r_filmgrain" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_film_grain_strength(strength);
            }
            "r_motionblur" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_motion_blur_strength(strength);
            }
            "r_depthoffield" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_depth_of_field_strength(strength);
            }
            "r_dofautofocus" => {
                self.video.dof_autofocus = boolean()?;
                self.crosshair_scan_at = Instant::now() - Duration::from_secs(1);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_dofquality" => {
                self.video.dof_quality = DofQuality::from_config(value)
                    .ok_or_else(|| format!("{name}: expected performance|adaptive|high"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_splittoning" => {
                self.video.split_toning.enabled = boolean()?;
                self.sync_post_effects(); self.mark_config_dirty(); self.publish_ui();
            }
            "r_splittoningstrength" => {
                self.video.split_toning.strength = finite_number()?.clamp(0.0, 1.0);
                self.sync_post_effects(); self.mark_config_dirty(); self.publish_ui();
            }
            "r_splittoningshadowhue" => {
                self.video.split_toning.shadow_hue = finite_number()?.clamp(0.0, 360.0);
                self.sync_post_effects(); self.mark_config_dirty(); self.publish_ui();
            }
            "r_splittoningshadowsaturation" => {
                self.video.split_toning.shadow_saturation = finite_number()?.clamp(0.0, 1.0);
                self.sync_post_effects(); self.mark_config_dirty(); self.publish_ui();
            }
            "r_splittoninghighlighthue" => {
                self.video.split_toning.highlight_hue = finite_number()?.clamp(0.0, 360.0);
                self.sync_post_effects(); self.mark_config_dirty(); self.publish_ui();
            }
            "r_splittoninghighlightsaturation" => {
                self.video.split_toning.highlight_saturation = finite_number()?.clamp(0.0, 1.0);
                self.sync_post_effects(); self.mark_config_dirty(); self.publish_ui();
            }
            "r_splittoningbalance" => {
                self.video.split_toning.balance = finite_number()?.clamp(-1.0, 1.0);
                self.sync_post_effects(); self.mark_config_dirty(); self.publish_ui();
            }
            "r_colorlut" => {
                let preset = ColorLutPreset::from_config(value)
                    .ok_or_else(|| format!("{name}: unknown LUT preset"))?;
                self.set_color_lut(preset);
            }
            "r_colorlutstrength" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_color_lut_strength(strength);
            }
            "r_gpudriven" | "r_hizocclusion" => {
                let enabled = boolean()?;
                if lower == "r_gpudriven" {
                    self.video.gpu_driven = enabled;
                } else {
                    self.video.hiz_occlusion = enabled;
                }
                self.sync_gpu_visibility();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_entityambientlighting" => {
                self.video.entity_ambient_lighting = EntityAmbientLightingMode::from_config(value)
                    .ok_or_else(|| {
                        format!("{name}: expected off|bsp_lightgrid|bevy_irradiance_volume")
                    })?;
                self.sync_entity_ambient_lighting();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_dynamiclights" => {
                self.video.dynamic_lights = DynamicLightsMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|legacy|vertex|clustered_lite|forward_plus|ray_traced"))?;
                self.sync_dynamic_lighting();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_dynamiclightfalloff" => {
                let mode = value
                    .parse::<u32>()
                    .ok()
                    .filter(|n| matches!(n, 0 | 1))
                    .ok_or_else(|| format!("{name}: expected 0|1"))?;
                self.video.dynamic_light_falloff = mode;
                self.render_command(RenderCommand::SetDynamicLightFalloff(mode));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_rtsamples" => {
                let samples = value
                    .parse::<u32>()
                    .ok()
                    .filter(|n| matches!(n, 1 | 2 | 4))
                    .ok_or_else(|| format!("{name}: expected 1|2|4"))?;
                self.video.rt_samples = samples;
                self.render_command(RenderCommand::SetRtSamples(samples));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_rtresolution" => {
                self.video.rt_half_resolution = match value.to_ascii_lowercase().as_str() {
                    "full" | "1" => false,
                    "half" | "0.5" => true,
                    _ => return Err(format!("{name}: expected full|half")),
                };
                self.render_command(RenderCommand::SetRtHalfResolution(
                    self.video.rt_half_resolution,
                ));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_fullbright" | "r_vertexlight" | "r_lightmap" => {
                let enabled = boolean()?;
                match lower.as_str() {
                    "r_fullbright" => self.video.world_lighting = !enabled,
                    "r_vertexlight" => self.video.vertex_lighting = enabled,
                    "r_lightmap" => self.video.lightmap_only = enabled,
                    _ => unreachable!(),
                }
                self.sync_classic_world_lighting();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_maplightsimulation" => {
                self.video.map_light_simulation = boolean()?;
                self.sync_map_light_simulation();
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    ".MAP LIGHT SIMULATION: {}",
                    if self.video.map_light_simulation {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r_modernsabers" => {
                self.video.modern_sabers = boolean()?;
                if let Some(session) = self.game_session.as_mut() {
                    session
                        .weapon_fx
                        .set_modern_sabers(self.video.modern_sabers);
                }
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "MODERN SABER RENDERING: {}",
                    if self.video.modern_sabers {
                        "ON"
                    } else {
                        "OFF (OPENJK)"
                    }
                );
            }
            "r_flares" => {
                self.video.flares = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status =
                    format!("FLARES: {}", if self.video.flares { "ON" } else { "OFF" });
            }
            "r_saberimpactfx" => {
                self.video.saber_impact_fx = boolean()?;
                if let Some(session) = self.game_session.as_mut() {
                    session
                        .weapon_fx
                        .set_saber_impact_fx(self.video.saber_impact_fx);
                }
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "SABER IMPACT EFFECTS: {}",
                    if self.video.saber_impact_fx {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
            }
            "r_sabermarks" => {
                self.video.saber_marks = ui::SaberMarkMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|legacy|enhanced"))?;
                if let Some(session) = self.game_session.as_mut() {
                    session.weapon_fx.set_saber_marks(self.video.saber_marks);
                }
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "SABER MARKS: {}",
                    self.video.saber_marks.label().to_ascii_uppercase()
                );
            }
            "r_dynamicshadows" => {
                self.video.dynamic_shadows = DynamicShadowsMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|blob|entity|csm|ray_traced"))?;
                self.video.cascaded_shadows =
                    self.video.dynamic_shadows == DynamicShadowsMode::CascadedShadowMaps;
                self.sync_cascaded_shadows();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_emissivearealights" => {
                self.video.emissive_area_lights = boolean()?;
                self.sync_emissive_area_lights();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_voxelprobegi" => {
                self.video.voxel_probe_gi = boolean()?;
                self.sync_voxel_probe_gi();
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "VOXEL / PROBE GI: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
                self.publish_ui();
            }
            "r_entityshadowlight" => {
                self.video.entity_shadow_light = EntityShadowLight::from_config(value)
                    .ok_or_else(|| format!("{name}: expected lightgrid|authored"))?;
                self.sync_entity_shadow_light();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_locallightshadows" => {
                self.video.local_light_shadows = boolean()?;
                self.sync_local_light_shadows();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_pbr" => {
                self.video.pbr = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "fs_allowassetoverrides" => {
                self.video.allow_asset_overrides = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_gennormalmaps" => {
                self.video.gen_normal_maps = boolean()?;
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "GENERATED NORMAL MAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART"
                            .into();
                }
                self.publish_ui();
            }
            "r_floatlightmap" => {
                self.video.float_lightmap = boolean()?;
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "FLOAT LIGHTMAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
                self.publish_ui();
            }
            "r_deluxemapping" => {
                self.video.deluxe_mapping = boolean()?;
                self.sync_pbr();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_deluxespecular" => {
                let scale = value
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                if !scale.is_finite() {
                    return Err(format!("{name}: expected 0..1"));
                }
                self.video.deluxe_specular = scale.clamp(0.0, 1.0);
                self.sync_pbr();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_cascadedshadows" => {
                self.video.cascaded_shadows = boolean()?;
                self.video.dynamic_shadows = if self.video.cascaded_shadows {
                    DynamicShadowsMode::CascadedShadowMaps
                } else {
                    DynamicShadowsMode::Off
                };
                self.sync_cascaded_shadows();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_planardebug" => {
                self.video.planar_reflection_debug = PlanarReflectionDebugMode::from_config(value)
                    .ok_or_else(|| {
                        format!("{name}: expected off|candidates|selected|texture|applied|binding")
                    })?;
                self.sync_planar_reflection_debug();
                self.publish_ui();
            }
            _ => return Err(format!("{name}: cvar is registered but has no setter yet")),
        }
        Ok(())
    }
}
