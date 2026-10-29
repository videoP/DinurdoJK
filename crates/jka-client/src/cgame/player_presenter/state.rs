//! State.
use crate::cgame::player_presenter::{
    footsteps, load_humanoid_animations, load_saber_animation_scales, load_saber_definitions,
    load_vehicle_definitions, materials, AnimationSet, Arc, AssetRegistry, AssetSearchPath,
    AssetSource, CosmeticDraw, DynamicModelAlphaMode, FootstepImpact, FootstepStages,
    Ghoul2Animator, Ghoul2PerfStats, Ghoul2SkinningMode, HashMap, HashSet, JiggleRemeshTuning,
    ModelLoadShared, PlayerFxRequest, PlayerPresenter, RagdollWorld, Textures, VehicleDefinition,
    VehicleDefinitions,
};

pub(in crate::cgame::player_presenter) fn resolve_vehicle_model_request(
    definitions: &VehicleDefinitions,
    requested: &str,
) -> Result<(String, Option<String>, String), String> {
    let (model_request, appended_skin) = match requested.rsplit_once('*') {
        Some((model, skin)) if !skin.is_empty() => (model, Some(skin)),
        Some((model, _)) => (model, None),
        None => (requested, None),
    };

    let (model_value, authored_skin, label) = if let Some(vehicle_name) =
        model_request.strip_prefix('$')
    {
        let definition = definitions.get(vehicle_name).ok_or_else(|| {
            format!("vehicle definition {model_request} was not found in ext_data/vehicles/*.veh")
        })?;
        (
            definition.model.as_str(),
            definition.skin.as_deref(),
            format!("{model_request} ({})", definition.model),
        )
    } else {
        (model_request, None, model_request.to_owned())
    };

    let normalized = model_value.replace('\\', "/");
    let glm = if normalized.to_ascii_lowercase().ends_with(".glm") {
        normalized
    } else if normalized.to_ascii_lowercase().starts_with("models/") {
        format!("{}/model.glm", normalized.trim_end_matches('/'))
    } else {
        format!("models/players/{}/model.glm", normalized.trim_matches('/'))
    };

    // OpenJK registers a vehicle's authored skin, or model_default.skin when
    // the .veh leaves `skin` blank. Do not treat the vehicle like a saber hilt
    // and rely on embedded surface shaders here.
    let skin_name = appended_skin.or(authored_skin).unwrap_or("default");
    let lower_glm = glm.to_ascii_lowercase();
    let skin = lower_glm.strip_suffix("model.glm").map(|_| {
        let prefix = &glm[..glm.len() - "model.glm".len()];
        format!("{prefix}model_{skin_name}.skin")
    });
    Ok((glm, skin, label))
}

impl PlayerPresenter {
    /// Connection ownership is independent of snapshot/playerState view ownership
    /// (a spectator can be following another player). Demo playback has no
    /// connection-owned body; explicit Profile previews set local_body themselves.
    pub(crate) fn set_local_body_owner(&mut self, owner: Option<usize>) {
        self.local_body_owner = Some(owner);
    }

    /// SoundPresenter -> S_GetVoiceVolume, sampled before CG_Player each frame.
    pub fn set_breath_talking(&mut self, entities: impl IntoIterator<Item = u16>) {
        self.breath_talking.clear();
        self.breath_talking.extend(entities);
    }

    /// Reset only per-entity/time-dependent presentation state for demo seeking.
    /// Asset/model caches stay hot so scrubbing does not turn into a reload path.
    pub fn reset_for_seek(&mut self) {
        self.fx_requests.clear();
        self.breath_puff_times.clear();
        self.breath_talking.clear();
        self.footsteps.clear();
        self.entities.clear();
        self.jiggle.clear();
        self.preview_source_jiggles.clear();
        self.team_power.clear();
        self.body_fade.clear();
        self.mind_trick_fades.clear();
        self.cloak_states.clear();
        self.force_refractions.clear();
        self.gore.clear();
        self.pending_gore.clear();
        self.last_gpu.clear();
        self.studio_bones.clear();
        self.studio_saber_tips.clear();
        self.vehicle_snaps.clear();
        self.vehicle_animations.clear();
        self.profile_bone_smooth_history = None;
        self.profile_body_pick = None;
        self.profile_selection = Default::default();
        self.force_grip_targets.clear();
        self.force_gripped_entities.clear();
        self.pending_impulse_ragdolls.clear();
        self.active_impulse_ragdolls.clear();
        self.explosion_impulse_pulses.clear();
        self.previous_impulse_velocity.clear();
        self.force_gesture_anim.clear();
        self.thrown_sabers.clear();
        self.body_queue_copies.clear();
        self.dismembered.clear();
        self.dismember_source_snaps.clear();
        self.detached_limb_visuals.clear();
        self.saber_blade_lengths.clear();
        self.blob_shadow_instances.clear();
        self.cloth.reset();
        self.ragdolls.reset_dynamic_for_seek();
        self.view_weapon_frame = 0;
        self.view_weapon_frame_time = 0;
    }

    /// Apply renderer-global R_RemapShader state at an asset-registration boundary.
    /// Model/material caches are rebuilt once here, so ordinary Ghoul2 draws do
    /// not perform shader-name remap lookups.
    pub(crate) fn set_shader_remaps(&mut self, remaps: crate::scene::ShaderRemapState) {
        if self.shader_remaps == remaps {
            return;
        }
        self.shader_remaps = remaps.clone();
        self.models.clear();
        self.saber_models.clear();
        self.vehicle_animations.clear();
        self.vehicle_snaps.clear();
        // Replacing registries drops the old generation token; queued jobs from
        // the previous material mapping cannot populate the new caches.
        self.async_models = AssetRegistry::new("player model");
        self.async_static_models = AssetRegistry::new("ghoul2 model");
        self.load_shared = Arc::new(ModelLoadShared::new(
            Arc::clone(&self.shaders),
            remaps,
        ));
        self.failed_models.clear();
        self.failed_saber_models.clear();
        self.prime_default_player_models();
    }

    pub fn vehicle_definition_for_model_request(
        &self,
        requested: &str,
    ) -> Option<&VehicleDefinition> {
        let model_request = requested
            .rsplit_once('*')
            .map_or(requested, |(model, _)| model);
        let vehicle_name = model_request.strip_prefix('$')?;
        self.vehicle_definitions.get(vehicle_name)
    }

    pub fn new(mut assets: AssetSearchPath, pbr: bool) -> Result<Self, String> {
        jka_assets::body_shape::anny::configure_assets(&assets);
        let animations = load_humanoid_animations(&mut assets)?;
        let anim_events =
            jka_assets::animevents::load_humanoid_animation_events(&mut assets, &animations)
                .unwrap_or_else(|error| {
                    println!("PLAYER ASSETS: no animation events, so no footsteps: {error}");
                    Default::default()
                });
        let saber_scales = load_saber_animation_scales(&mut assets)?;
        let saber_definitions = load_saber_definitions(&mut assets)?;
        let vehicle_definitions = load_vehicle_definitions(&mut assets)?;
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut assets, &mut shader_warnings, pbr)?;
        devprintln!(
            1,
            "PLAYER ASSETS: humanoid animations={} saberDefs={} vehicleDefs={} shaderDefs={} mtrDefs={}",
            animations.animations.len(),
            saber_definitions.len(),
            vehicle_definitions.len(),
            diagnostics.shader_definitions,
            diagnostics.mtr_definitions,
        );
        for warning in shader_warnings {
            rverbose!(1, "PLAYER MATERIAL WARNING: {warning}");
        }
        let asset_source = AssetSource::from_search_path(&assets);
        let shaders = Arc::new(shaders);
        let shader_remaps = crate::scene::ShaderRemapState::default();
        let load_shared = Arc::new(ModelLoadShared::new(
            Arc::clone(&shaders),
            shader_remaps.clone(),
        ));
        let worker_count = std::thread::available_parallelism()
            .map_or(4, |count| count.get())
            .saturating_sub(2)
            .clamp(1, 8);
        let skinning_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .thread_name(|index| format!("jka-g2-skin-{index}"))
            .build()
            .map_err(|error| format!("could not start Ghoul2 skinning workers: {error}"))?;
        devprintln!(1, "Ghoul2 skinning worker pool: {worker_count} thread(s)");
        let mut presenter = Self {
            assets,
            pbr,
            shaders,
            shader_remaps,
            textures: Textures::new(),
            texture_arcs: HashMap::new(),
            animations,
            saber_scales,
            saber_definitions,
            vehicle_definitions,
            models: HashMap::new(),
            saber_models: HashMap::new(),
            async_models: AssetRegistry::new("player model"),
            async_static_models: AssetRegistry::new("ghoul2 model"),
            asset_source,
            load_shared,
            async_loading: true,
            fx_requests: Vec::new(),
            breath_puff_times: HashMap::new(),
            breath_talking: HashSet::new(),
            cold_breath_effects: false,
            breath_view_contents: 0,
            anim_events,
            footstep_stages: FootstepStages::from_level(footsteps::FOOTSTEPS_MARKS),
            footsteps: Vec::new(),
            footstep_rng: 0x2545_f491,
            stage_time_ms: 0,
            stage_view_position: None,
            perf: Ghoul2PerfStats::default(),
            viewer_client: -1,
            viewer_foot_bolts: None,
            studio_bone_masks:HashMap::new(),studio_bones:HashMap::new(),
            studio_saber_clients:Vec::new(),studio_saber_tips:HashMap::new(),studio_fpls:false,
            viewer_anim_debug: None,
            viewer_dueling: false,
            viewer_force_see: false,
            cloak_states: HashMap::new(),
            force_refractions: HashMap::new(),
            force_sight_level: 0,
            force_sight_team: 0,
            force_sight_gametype: 0,
            japro: crate::japro_cg::JaproCgame::default(),
            viewer_style: crate::japro_cg::StyleViewer::default(),
            mind_trick_fades: HashMap::new(),
            look: crate::japro_cg::Appearance::NORMAL,
            japro_cinfo2: 0,
            head_look_mode: 1,
            swim_tilt: 0.9,
            swim_turn_bank: 1.0,
            move_lean: 0.75,
            saber_team_colors: true,
            saber_staff_multi_color: false,
            duel_shell_gray: None,
            cosmetic_draws: Vec::new(),
            emote_fx: Default::default(),
            local_cosmetics: 0,
            local_global_hat: String::new(),
            local_global_cape: String::new(),
            global_cosmetic_offsets: HashMap::new(),
            packed_cosmetic_assets: None,
            sent_cosmetics: 0,
            cosmetic_mismatch_logged: None,
            team_power: HashMap::new(),
            body_fade: HashMap::new(),
            gore_limit: 0,
            gore: Vec::new(),
            pending_gore: Vec::new(),
            gore_next_id: 1,
            last_gpu: HashMap::new(),
            vehicle_snaps: HashMap::new(),
            vehicle_animations: HashMap::new(),
            entities: HashMap::new(),
            external_live_entities: HashSet::new(),
            local_body_owner: None,
            jiggle: super::AsyncJiggleSystem::default(),
            preview_source_jiggles: HashMap::new(),
            jiggle_sex_filter: 0,
            hair_physics_mode: 2,
            jiggle_remesh: JiggleRemeshTuning::default(),
            cloth: super::AsyncClothSystem::default(),
            cloth_weather_wind: None,
            cloth_body_templates: HashMap::new(),
            ragdolls: RagdollWorld::new(),
            force_grip_targets: HashMap::new(),
            force_gripped_entities: HashSet::new(),
            pending_impulse_ragdolls: HashMap::new(),
            active_impulse_ragdolls: HashMap::new(),
            explosion_impulse_pulses: Vec::new(),
            previous_impulse_velocity: HashMap::new(),
            force_gesture_anim: HashMap::new(),
            thrown_sabers: HashMap::new(),
            body_queue_copies: HashMap::new(),
            dismembered: HashMap::new(),
            dismember_source_snaps: HashMap::new(),
            detached_limb_visuals: HashMap::new(),
            saber_blade_lengths: HashMap::new(),
            failed_models: HashMap::new(),
            player_diagnostics: HashMap::new(),
            failed_saber_models: HashSet::new(),
            reported_saber_warnings: HashSet::new(),
            reported_saber_diagnostics: HashSet::new(),
            reported_vehicle_fallbacks: HashSet::new(),
            logged_first_draw: false,
            skinning_mode: Ghoul2SkinningMode::Gpu,
            preview_cpu_remesh: false,
            profile_body_pick: None,
            profile_selection: Default::default(),
            early_frustum_cull: true,
            rt_shadow_casters_enabled: false,
            collision_world: None,
            blob_shadow_opacity: 0.0,
            blob_shadow_contact: false,
            blob_shadow_instances: Vec::new(),
            blob_shadow_texture: None,
            blob_shadow_alpha_mode: DynamicModelAlphaMode::BlendUnlit,
            blob_shadow_texture_resolved: false,
            blob_shadow_asset_warned: false,
            lod_bias: 0,
            lod_scale: crate::fx::LOD_SCALE_DEFAULT,
            ghoul2_anim_smooth: 0.3,
            profile_bone_smooth_history: None,
            studio_pose_blend: None,
            view_weapon_frame: 0,
            view_weapon_frame_time: 0,
            skinning_pool,
        };
        presenter.prime_default_player_models();
        Ok(presenter)
    }

    /// Applies to player models in live games, demo playback and solo sessions.
    /// Only the visual look vector is changed, not entityState or Pmove.
    pub fn set_head_look_mode(&mut self, mode: u8) {
        self.head_look_mode = mode.min(2);
    }

    /// Visual-only, including remote players; 0 returns to exact stock orientation.
    pub fn set_swim_tilt(&mut self, strength: f32) {
        self.swim_tilt = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 0.0 };
        if self.swim_tilt == 0.0 {
            for runtime in self.entities.values_mut() {
                runtime.swim_angles = [0.0; 2];
                runtime.swim_bank_angle = 0.0;
                runtime.swim_angle_time = 0;
                runtime.swim_turn_motion.reset();
            }
        }
    }

    /// Cosmetic banking into actual swim velocity curves. The master
    /// cg_swimTilt still controls whether any presentation tilt is applied.
    pub fn set_swim_turn_bank(&mut self, strength: f32) {
        self.swim_turn_bank = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    /// Cosmetic air-only strafejump/bunnyhop lean (ground run/walk remains unchanged).
    pub fn set_move_lean(&mut self, strength: f32) {
        self.move_lean = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 0.0 };
        if self.move_lean == 0.0 {
            for runtime in self.entities.values_mut() {
                runtime.move_lean.reset(self.stage_time_ms);
            }
        }
    }

    /// TaystJK `CG_MapTorsoToWeaponFrame`: sample the already-advanced local
    /// Ghoul2 lower_lumbar animation and map it onto the MD3 hand animation.
    /// The returned `(frame, oldframe, backlerp)` is consumed by the viewmodel
    /// tag interpolation exactly like refEntity_t.
    pub fn view_weapon_frames(
        &mut self,
        entity_num: u16,
        current_time: i32,
        torso_anim: i32,
        force_hand_extend: i32,
    ) -> (usize, usize, f32) {
        const HANDEXTEND_NONE: i32 = 0;

        // WEAPON_FORCE_BUSY_HOLSTER from TaystJK: advance frames 6..10 at
        // 10 ms steps while a hand-extend is active, then retain frame 10 for
        // 100 ms so the weapon cannot snap to idle for one render frame.
        if force_hand_extend != HANDEXTEND_NONE || self.view_weapon_frame_time > current_time {
            if self.view_weapon_frame < 6 {
                self.view_weapon_frame = 6;
                self.view_weapon_frame_time = current_time + 10;
            } else if self.view_weapon_frame_time < current_time && self.view_weapon_frame < 10 {
                self.view_weapon_frame += 1;
                self.view_weapon_frame_time = current_time + 10;
            } else if force_hand_extend != HANDEXTEND_NONE && self.view_weapon_frame == 10 {
                self.view_weapon_frame_time = current_time + 100;
            }
            let frame = self.view_weapon_frame.max(0) as usize;
            return (frame, frame, 0.0);
        }
        self.view_weapon_frame = 0;
        self.view_weapon_frame_time = 0;

        let current_frame = self.entities.get_mut(&entity_num).and_then(|state| {
            let lower_lumbar = Ghoul2Animator::bone_index(&state.model.animation_gla, "lower_lumbar")?;
            state
                .animation
                .animator
                .bone_frame(&state.model.animation_gla, lower_lumbar, current_time)
                .ok()
                .flatten()
        });
        let Some(current_frame) = current_frame else {
            return (0, 0, 0.0);
        };

        let Some(animation) = self.animations.get(torso_anim) else {
            return (0, 0, 0.0);
        };
        let first = i32::from(animation.first_frame);
        let map = |frame: i32| -> Option<usize> {
            let offset = frame - first;
            match AnimationSet::name(torso_anim) {
                Some("TORSO_DROPWEAP1") if (0..5).contains(&offset) => Some((offset + 6) as usize),
                Some("TORSO_RAISEWEAP1") if (0..4).contains(&offset) => {
                    Some((offset + 10) as usize)
                }
                Some(
                    "BOTH_ATTACK1" | "BOTH_ATTACK2" | "BOTH_ATTACK3" | "BOTH_ATTACK4"
                    | "BOTH_ATTACK10" | "BOTH_THERMAL_THROW",
                ) if (0..6).contains(&offset) => Some((offset + 1) as usize),
                _ => None,
            }
        };

        let frame = map(current_frame.ceil() as i32);
        let oldframe = map(current_frame.floor() as i32);
        match (frame, oldframe) {
            (None, _) => (0, 0, 0.0),
            (Some(frame), None) => (frame, frame, 0.0),
            (Some(frame), Some(oldframe)) => (frame, oldframe, 1.0 - current_frame.fract()),
        }
    }

    /// `cg_footsteps` picks the sound and dust stages; prints follow the
    /// Footprints video setting alone (`prints`).
    pub fn set_footstep_level(&mut self, level: u8, prints: bool) {
        self.footstep_stages = FootstepStages {
            marks: prints,
            ..FootstepStages::from_level(level)
        };
    }

    pub fn footstep_stages(&self) -> FootstepStages {
        self.footstep_stages
    }

    /// Footfalls that found ground since the last call.
    pub fn drain_footsteps(&mut self) -> Vec<FootstepImpact> {
        std::mem::take(&mut self.footsteps)
    }

    /// Force-power effect requests produced since the last call.
    pub fn drain_fx_requests(&mut self) -> Vec<PlayerFxRequest> {
        std::mem::take(&mut self.fx_requests)
    }

    /// Cosmetic (hat/cape) MD3 submissions produced since the last call. The
    /// entity presenter owns MD3 loading, so the app forwards these to it.
    pub fn drain_cosmetic_draws(&mut self) -> Vec<CosmeticDraw> {
        std::mem::take(&mut self.cosmetic_draws)
    }

    /// Baked emote particle sprites/ribbons and lights produced since the
    /// last call. Mesh particles are already in the cosmetic draws.
    pub fn drain_emote_fx(&mut self) -> (Vec<crate::fx::system::FxDraw>, Vec<crate::fx::system::FxLight>) {
        let frame = std::mem::take(&mut self.emote_fx);
        (frame.draws, frame.lights)
    }

    /// The viewer's own `cp_cosmetics` in a local game, where there is no server
    /// to relay it back as `c5`.
    pub fn set_local_cosmetics(&mut self, mask: u32) {
        self.local_cosmetics = mask;
    }

    /// Local global cosmetics for solo preview and servers that do not echo
    /// our own userinfo; network peers still use the received c1/c2 suffix.
    pub fn set_local_global_cosmetics(&mut self, hat: &str, cape: &str) {
        self.local_global_hat = hat.to_owned();
        self.local_global_cape = cape.to_owned();
    }

    pub fn set_saber_team_colors(&mut self, enabled: bool) {
        self.saber_team_colors = enabled;
    }

    pub fn set_saber_staff_multi_color(&mut self, enabled: bool) {
        self.saber_staff_multi_color = enabled;
    }

    /// The `cp_cosmetics` this client sends a jaPRO server (diagnostics only).
    pub fn set_sent_cosmetics(&mut self, mask: u32) {
        self.sent_cosmetics = mask;
    }

    /// jaPRO client options (`cg_stylePlayer` and friends).
    pub fn set_japro_options(&mut self, options: crate::japro_cg::JaproCgame) {
        self.japro = options;
    }

    pub fn begin_perf_frame(&mut self) {
        self.perf = Ghoul2PerfStats::default();
    }
}
