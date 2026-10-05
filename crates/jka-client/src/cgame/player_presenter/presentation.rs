//! Presentation.
use crate::cgame::player_presenter::{
    angles_to_axis, bg_g2_player_angles, dismember_source_entity, dismember_source_ready,
    find_siege_class_visual, flatten_matrix3x4, foot_bolts_timed, footsteps,
    fpls_mode3_surface_hidden, ghoul2_entity_radius, ghoul2_lod_for_view, ghoul2_model_scale,
    ghoul2_render_origin, ghoul2_render_transform, model_bolt_matrix_timed,
    model_bolt_origin_timed, multiply_3x4, openjk_look_target_origin,
    openjk_player_entity_needs_reset, openjk_vectoangles, player_angle_entity, player_entity_rgb,
    record_motion_pose_eval, record_pose_eval, reset_remote_player_saber_attachment,
    smooth_ghoul2_pose, source_dismember_surface_set, suppressed_during_intermission,
    transform_jka_model_point, update_weapon_attachment, viewer_style_from, weapon_world_model,
    Arc, ClientGameState, DismemberPart, DismemberSourceSnap, DynamicModelAlphaMode,
    DynamicModelSurface, EntityPlayerState, FootstepStages, ForcedPlayerModels, Ghoul2Animator,
    Ghoul2PresentationView, HashMap, HashSet, Instant, ModelResolution, PlayerAngleState,
    PlayerAnimationState, PlayerPresenter, PresentedEntity, RagdollMode, SaberBladeLengthKey,
    SaberBladeLengthState, ViewerAnimDebug, BONE_ANGLES_POSTMULT, DEFAULT_SABER_BLADE_LENGTH_MAX,
    EF2_SHIP_DEATH, EF_DEAD, EF_JETPACK, EF_NODRAW, EF_RAG, ET_BODY, ET_NPC, ET_PLAYER, GT_SIEGE,
    MAX_CLIENTS, MAX_SABER_BLADES, TEAM_SPECTATOR, WP_SABER,
};

impl PlayerPresenter {
    pub fn present_snapshot_players(
        &mut self,
        entities: &[PresentedEntity],
        game: &ClientGameState,
        siege_classes: &[jka_assets::siege::SiegeClassVisual],
        current_time: i32,
        preserve_entity: Option<u16>,
        hidden_first_person_entity: Option<u16>,
        view: Option<Ghoul2PresentationView>,
        forced_models: Option<&ForcedPlayerModels>,
    ) -> Vec<DynamicModelSurface> {
        self.stage_time_ms = current_time;
        self.japro_cinfo2 = game.japro_cinfo2();
        self.cloth.begin_frame(current_time);
        self.stage_view_position = view.map(|view| view.position.to_array());
        if let Some(snapshot) = game.current_snapshot() {
            let viewer_client = snapshot.player_state.field_i32("clientNum").unwrap_or(-1);
            if viewer_client != self.viewer_client {
                self.viewer_foot_bolts = None;
                self.viewer_anim_debug = None;
            }
            self.viewer_client = viewer_client;
            self.viewer_dueling = snapshot
                .player_state
                .field_i32("duelInProgress")
                .unwrap_or(0)
                != 0;
            self.viewer_style = viewer_style_from(
                &snapshot.player_state,
                game.is_japro(),
                self.japro.style_player,
            );
        }
        // Keep the disabled path effectively free: only parse the viewer's
        // ClientInfo when a force-model override is actually enabled.
        let viewer_info = forced_models.and_then(|_| {
            usize::try_from(self.viewer_client)
                .ok()
                .and_then(|client| game.client_info(client, siege_classes))
        });
        let mut draws = Vec::new();
        let mut live_entities = HashSet::new();
        let mut live_ragdolls = HashSet::new();
        let mut live_detached_limbs = HashSet::new();
        let mut detached_entities = Vec::new();
        if let Some(entity_num) = preserve_entity {
            // The local/followed player is presented separately just like
            // OpenJK's predictedPlayerEntity, but it still owns persistent
            // centity/playerEntity state across frames.
            live_entities.insert(entity_num);
        }
        live_entities.extend(self.external_live_entities.iter().copied());
        // CG_Player resolves lookTarget against cg_entities before BG_G2PlayerAngles.
        let entity_origins = entities
            .iter()
            .map(|entity| (entity.number, entity.origin))
            .collect::<HashMap<_, _>>();
        self.duel_shell_gray = self.duel_shell_brightness(game, &entity_origins, preserve_entity);

        // Authoritative dismemberment is transmitted as ET_GENERAL/G2_MODEL_PART.
        // Apply the source surface mutation before CG_Player draws the corpse,
        // then draw the detached copies after source poses have been evaluated.
        let dismemberment_level = self.ragdolls.dismemberment_level();
        if dismemberment_level == 0 {
            self.dismembered.clear();
            self.dismember_source_snaps.clear();
            self.detached_limb_visuals.clear();
        } else {
            // OpenJK CG_ReattachLimb restores a living client instance.
            for entity in entities
                .iter()
                .filter(|entity| entity.entity_type == ET_PLAYER || entity.entity_type == ET_NPC)
            {
                if entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0 {
                    self.dismembered.remove(&entity.number);
                }
            }
            for entity in entities {
                let Some(source) = dismember_source_entity(entity) else {
                    continue;
                };
                let Some(part) = DismemberPart::from_model_part(
                    entity.state.field_i32("modelGhoul2").unwrap_or(0),
                ) else {
                    continue;
                };
                if !part.allowed_at_level(dismemberment_level) {
                    continue;
                }
                let Some(source_entity) =
                    entities.iter().find(|candidate| candidate.number == source)
                else {
                    continue;
                };
                // Match CG_General's creation gate before mutating the source
                // surfaces. At this prepass point the cached torso animation is
                // from the last presented frame; if no runtime exists yet, wait
                // rather than cutting a source before its death pose is valid.
                let presented_torso_anim = self
                    .entities
                    .get(&source)
                    .map(|runtime| runtime.animation.torso.animation_number);
                if !dismember_source_ready(source_entity, presented_torso_anim) {
                    continue;
                }
                self.dismembered.entry(source).or_default().insert(part);
                live_detached_limbs.insert(entity.number);
                detached_entities.push(entity);
            }
        }

        // ET_PLAYER uses CG_Player and ET_NPC uses CG_G2Animated. ET_BODY is
        // CG_General in OpenJK; this Rust presenter still owns the shared body
        // mesh/ragdoll submission, but its weapon state comes only from ircg.
        // NPCs resolve `ci = cent->npcClient` instead of cgs.clientinfo[].
        let intermission = game.rendering_intermission();
        for entity in entities.iter().filter(|entity| {
            entity.entity_type == ET_PLAYER
                || entity.entity_type == ET_NPC
                || entity.entity_type == ET_BODY
        }) {
            // Hidden intermission entities remain live centities in TaystJK;
            // only their CG_AddCEntity presentation is skipped. Preserve the
            // cached Ghoul2 owner while withholding its draw/update work.
            live_entities.insert(entity.number);
            // TaystJK CG_AddCEntity suppresses ET_PLAYER outright during
            // intermission and suppresses ET_NPC only for vehicles. ET_BODY
            // intentionally remains eligible, matching the original switch.
            if suppressed_during_intermission(intermission, entity) {
                continue;
            }
            let entity_eflags = entity.state.field_i32("eFlags").unwrap_or(0);
            if entity.entity_type == ET_BODY
                || entity_eflags & (EF_DEAD | EF_RAG) != 0
                || self.force_gripped_entities.contains(&entity.number)
            {
                live_ragdolls.insert(entity.number);
            }

            // OpenJK CG_AddPacketEntities first synthesizes/adds the predicted
            // player from playerState, then explicitly skips that same client
            // number while walking snapshot entities. `preserve_entity` is our
            // separately-presented local/followed player, so presenting the
            // snapshot copy here as well queues a second SaberBlade for the same
            // (entity,saber,blade) key. If the two poses disagree about wall
            // contact, the later copy clears haveOldPos every frame and saber
            // marks can never connect. Keep it live, but do not present it twice.
            if preserve_entity == Some(entity.number) {
                continue;
            }

            // CG_Player returns before animating or submitting anything for
            // EF_NODRAW (e.g. NPCs a spawner is still holding, parked with
            // anim 0) and EF2_SHIP_DEATH; the centity itself stays alive.
            if entity.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW != 0
                || entity.state.field_i32("eFlags2").unwrap_or(0) & EF2_SHIP_DEATH != 0
            {
                continue;
            }
            self.look = crate::japro_cg::Appearance::NORMAL;
            if entity.entity_type == ET_PLAYER {
                self.look = crate::japro_cg::classify(
                    &self.viewer_style,
                    i32::from(entity.number),
                    entity.state.field_i32("bolt1").unwrap_or(0),
                    entity_eflags & EF_DEAD != 0,
                );
                if !self.look.visible {
                    // CG_Player returns before animating, shadowing or adding
                    // sprites: dueling/racing bystanders simply do not exist.
                    continue;
                }
            } else if entity.entity_type == ET_BODY
                && self.viewer_style.dueling
                && self.viewer_style.japro
            {
                // CG_General hides corpses from a jaPRO duelist's view.
                continue;
            }
            if entity.entity_type != ET_BODY {
                self.queue_blob_shadow(entity);
            }
            if entity.entity_type == ET_NPC
                && entity.state.field_i32("NPC_class").unwrap_or(0) == crate::cgame::CLASS_VEHICLE
            {
                match self.present_vehicle_static(entity, game, current_time, view) {
                    Ok(mut vehicle_draws) => {
                        self.report_player_status(
                            entity.number,
                            format!(
                                "vehicle submitted={}",
                                usize::from(!vehicle_draws.is_empty())
                            ),
                        );
                        draws.append(&mut vehicle_draws);
                    }
                    Err(error) => self.report_player_status(
                        entity.number,
                        format!("vehicle submitted=0 reason={error}"),
                    ),
                }
                continue;
            }

            let (client_num, info) = if entity.entity_type == ET_NPC {
                match game.npc_client_info(&entity.state) {
                    Ok(info) => (usize::from(entity.number), info),
                    Err(reason) => {
                        self.report_player_status(
                            entity.number,
                            format!("npc submitted=0 reason={reason}"),
                        );
                        continue;
                    }
                }
            } else {
                let client_num = entity
                    .state
                    .field_i32("clientNum")
                    .unwrap_or(i32::from(entity.number));
                let Ok(client_num) = usize::try_from(client_num) else {
                    self.report_player_status(
                        entity.number,
                        format!("clientNum={client_num} submitted=0 reason=invalid-client"),
                    );
                    continue;
                };
                let Some(mut info) = game.client_info(client_num, siege_classes) else {
                    self.report_player_status(
                        entity.number,
                        format!("clientNum={client_num} submitted=0 reason=missing-client-info"),
                    );
                    continue;
                };
                if let Some(forced) = forced_models {
                    forced.apply_to(&mut info, viewer_info.as_ref());
                    // TaystJK applies Siege class forcedModel/forcedSkin after
                    // cg_forceModel, so class-required visuals remain authoritative.
                    if info.gametype == GT_SIEGE {
                        if let Some(class) =
                            find_siege_class_visual(siege_classes, &info.siege_class)
                        {
                            if !class.forced_model.is_empty() {
                                info.model_name.clone_from(&class.forced_model);
                            }
                            if !class.forced_skin.is_empty() {
                                info.skin_name.clone_from(&class.forced_skin);
                            }
                        }
                    }
                }
                (client_num, info)
            };

            let look_target_origin = openjk_look_target_origin(entity, &entity_origins);
            self.queue_player_sprites(
                entity,
                game,
                current_time,
                hidden_first_person_entity != Some(entity.number),
            );
            match self.present_player(
                entity,
                &info,
                current_time,
                1.0,
                look_target_origin,
                false,
                hidden_first_person_entity != Some(entity.number),
                false,
                view,
            ) {
                Ok(mut player_draws) => draws.append(&mut player_draws),
                Err(error) => {
                    self.report_player_status(
                        entity.number,
                        format!(
                            "clientNum={client_num} requested={}/{} submitted=0 reason={error}",
                            info.model_name, info.skin_name,
                        ),
                    );
                }
            }

            self.look = crate::japro_cg::Appearance::NORMAL;
            if entity.entity_type != ET_BODY {
                match self.present_thrown_saber_for_player(
                    entity,
                    &info,
                    entities,
                    game,
                    current_time,
                    1.0,
                ) {
                    Ok(mut saber_draws) => draws.append(&mut saber_draws),
                    Err(error) => self.report_saber_warning_once(entity.number, &error),
                }
            }
        }

        // G2_MODEL_PART is created by the server, but its old ExPhys motion is
        // only 20 Hz. Clone the exact posed Ghoul2 subtree and hand one compact
        // rigid body to Rapier; the server entity still decides that the sever
        // happened and which part it was.
        for limb in detached_entities {
            let source_num = dismember_source_entity(limb);
            let source = source_num
                .and_then(|source| entities.iter().find(|entity| entity.number == source));
            match self.present_detached_limb(limb, source, current_time, view) {
                Ok(mut limb_draws) => draws.append(&mut limb_draws),
                Err(error) => self.report_player_status(
                    limb.number,
                    format!("dismember submitted=0 reason={error}"),
                ),
            }
        }
        self.ragdolls.retain_detached_limbs(&live_detached_limbs);
        self.detached_limb_visuals
            .retain(|entity, _| live_detached_limbs.contains(entity));

        // The followed/local player may not exist in packet entities but can
        // still be an exact forceGripCripple victim. Keep that live ragdoll
        // until present_player_entity runs later in the frame.
        live_ragdolls.extend(self.force_gripped_entities.iter().copied());
        live_ragdolls.extend(self.active_impulse_ragdolls.keys().copied());
        self.entities
            .retain(|entity_num, _| live_entities.contains(entity_num));
        self.jiggle.retain_entities(&live_entities);
        let mut live_dismember_sources = live_entities.clone();
        live_dismember_sources.extend(
            self.detached_limb_visuals
                .values()
                .map(|visual| visual.source_entity),
        );
        self.dismember_source_snaps
            .retain(|entity_num, _| live_dismember_sources.contains(entity_num));
        self.dismembered
            .retain(|entity_num, _| live_dismember_sources.contains(entity_num));
        self.ragdolls.retain_visible(&live_ragdolls);
        self.thrown_sabers
            .retain(|entity_num, _| live_entities.contains(entity_num));
        self.player_diagnostics
            .retain(|entity_num, _| live_entities.contains(entity_num));
        if !self.logged_first_draw && !draws.is_empty() {
            devprintln!(
                1,
                "PLAYER DRAW READY: players={} surfaces={}",
                live_entities.len(),
                draws.len(),
            );
            self.logged_first_draw = true;
        }
        draws
    }

    /// Present one local/followed player through the same Ghoul2/model/animation
    /// path used for snapshot players. This is deliberately not a second
    /// third-person renderer.
    pub fn present_player_entity(
        &mut self,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        current_time: i32,
        entity_alpha: f32,
        look_target_origin: Option<[f32; 3]>,
        force_reset: bool,
        submit_geometry: bool,
        first_person_saber: bool,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let mut entity_alpha = entity_alpha;
        self.look = crate::japro_cg::Appearance::NORMAL;
        if entity.entity_type == ET_BODY {
            if let Some(&started) = self.body_fade.get(&entity.number) {
                // CG_Player: alpha = 254 - elapsed * 0.08, invisible below 1.
                let byte = 254.0 - (current_time - started).max(0) as f32 * 0.08;
                if byte < 1.0 {
                    return Ok(Vec::new());
                }
                entity_alpha *= byte / 255.0;
            }
        }
        if entity.entity_type != ET_BODY
            && entity.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW == 0
            && entity.state.field_i32("eFlags2").unwrap_or(0) & EF2_SHIP_DEATH == 0
        {
            self.queue_blob_shadow(entity);
        }
        if entity.entity_type == ET_PLAYER && i32::from(entity.number) == self.viewer_client {
            self.viewer_anim_debug = None;
        }
        let result = self.present_player(
            entity,
            info,
            current_time,
            entity_alpha,
            look_target_origin,
            force_reset,
            submit_geometry,
            first_person_saber,
            view,
        );
        if entity.entity_type == ET_PLAYER && i32::from(entity.number) == self.viewer_client {
            let surfaces = result.as_ref().map_or(0, |draws| draws.len() as u32);
            let debug = self
                .viewer_anim_debug
                .get_or_insert_with(ViewerAnimDebug::default);
            debug.call_time = current_time;
            debug.submit_geometry = submit_geometry;
            debug.surfaces = surfaces;
        }
        result
    }

    /// Present a visual-only race ghost through the same player model/animation
    /// path without player sprites, blob shadows, gameplay classification, or RT
    /// shadow casting. Geometry still uses the normal depth test and view culling.
    pub fn present_race_ghost_entity(
        &mut self,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        current_time: i32,
        entity_alpha: f32,
        force_reset: bool,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let previous_look = self.look;
        self.look = crate::japro_cg::Appearance::NORMAL;
        let footstep_count = self.footsteps.len();
        let fx_request_count = self.fx_requests.len();
        let footstep_stages = self.footstep_stages;
        self.footstep_stages = FootstepStages {
            sounds: false,
            effects: false,
            marks: false,
        };
        let result = self.present_player(
            entity,
            info,
            current_time,
            entity_alpha.clamp(0.02, 1.0),
            None,
            force_reset,
            true,
            false,
            view,
        );
        self.look = previous_look;
        // The ghost is deliberately silent and visual-only. Do not even run
        // footstep ground traces, and let no animation/force FX escape to the
        // shared event/audio/WeaponFx queues. Cosmetic model draws are retained.
        self.footstep_stages = footstep_stages;
        self.footsteps.truncate(footstep_count);
        self.fx_requests.truncate(fx_request_count);
        let mut draws = result?;
        for draw in &mut draws {
            draw.rt_rigid = None;
            draw.rt_skinned_key = None;
        }
        Ok(draws)
    }

    pub(in crate::cgame::player_presenter) fn present_player(
        &mut self,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        current_time: i32,
        entity_alpha: f32,
        look_target_origin: Option<[f32; 3]>,
        force_reset: bool,
        submit_geometry: bool,
        first_person_saber: bool,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        self.poll_asset_completions();

        // CG_NewClientInfo/CG_RegisterClientModelname normalize team presentation
        // before model registration. Do the same here after any cg_forceModel
        // rewrite, so model caching and async requests see the final team skin.
        let mut team_info = info.clone();
        if team_info.gametype >= crate::cgame::GT_TEAM
            && team_info.gametype != crate::cgame::GT_SIEGE
            && !team_info.jedi_v_merc
        {
            team_info.team_color_override = crate::cgame::validate_skin_for_team(
                &team_info.model_name,
                &mut team_info.skin_name,
                team_info.team,
                |qpath| self.assets.contains_qpath(qpath),
            );
        } else {
            team_info.team_color_override = None;
        }
        let info = &team_info;

        let cached_model = self
            .entities
            .get(&entity.number)
            .filter(|runtime| {
                runtime.model_settled
                    && runtime
                        .requested_model_name
                        .eq_ignore_ascii_case(&info.model_name)
                    && runtime
                        .requested_skin_name
                        .eq_ignore_ascii_case(&info.skin_name)
            })
            .map(|runtime| Arc::clone(&runtime.model));
        let (model, model_settled) = if let Some(model) = cached_model {
            (model, true)
        } else if !self.async_loading_enabled() {
            (self.load_model(info)?, true)
        } else {
            // Request the real model immediately but never wait for it: keep
            // the entity's previous model, or the resident default body for a
            // new player, until the worker finishes; then swap automatically.
            match self.resolve_player_model_async(info) {
                ModelResolution::Ready(model) => (model, true),
                ModelResolution::Provisional(model) => match self.entities.get(&entity.number) {
                    Some(runtime) => (Arc::clone(&runtime.model), false),
                    None => (model, false),
                },
                ModelResolution::Loading => match self.entities.get(&entity.number) {
                    Some(runtime) => (Arc::clone(&runtime.model), false),
                    None => return Ok(Vec::new()),
                },
                ModelResolution::Failed(error) => match self.entities.get(&entity.number) {
                    // Keep showing the last valid model rather than vanishing.
                    Some(runtime) => (Arc::clone(&runtime.model), true),
                    None => return Err(error),
                },
            }
        };

        // OpenJK loads a client's selected saber definitions/models as part of
        // client/Ghoul2 setup, not only on the first frame the saber happens to
        // be visible. Keep the assets cached up front and let currentState.weapon
        // decide whether they are actually attached/rendered this frame. Server
        // weapon-disable rules affect what the server grants; they must not hide
        // a weapon that is explicitly present in a snapshot.
        self.register_client_saber_assets(info, entity.number);

        let mut recreate = self
            .entities
            .get(&entity.number)
            .is_none_or(|runtime| runtime.model_key != model.key);
        if recreate {
            // A model swap onto the very same skeleton (all humanoid players
            // share one GLA through the asset workers) keeps the running
            // animation/angle state instead of restarting the player.
            if let Some(runtime) = self
                .entities
                .get_mut(&entity.number)
                .filter(|runtime| Arc::ptr_eq(&runtime.model.gla, &model.gla))
            {
                runtime.model_key.clone_from(&model.key);
                runtime.model = Arc::clone(&model);
                recreate = false;
            }
        }
        if recreate {
            let animation = PlayerAnimationState::new(&model.gla, current_time)?;
            self.entities.insert(
                entity.number,
                EntityPlayerState {
                    model_key: model.key.clone(),
                    model_settled,
                    requested_model_name: info.model_name.clone(),
                    requested_skin_name: info.skin_name.clone(),
                    model: Arc::clone(&model),
                    animation,
                    player_angles: PlayerAngleState::reset_from_angles(entity.angles),
                    last_angle_time: current_time,
                    last_e_flags: entity.state.field_i32("eFlags").unwrap_or(0),
                    last_client_num: entity
                        .state
                        .field_i32("clientNum")
                        .unwrap_or(i32::from(entity.number)),
                    ghoul2_weapon: None,
                    cent_weapon: 0,
                    attached_weapon: None,
                    primary_saber_attached: false,
                    secondary_saber_attached: false,
                    saber_names: [info.saber_name.clone(), info.saber2_name.clone()],
                    bone_smooth_history: None,
                    bone_smooth_time: current_time,
                },
            );
        }

        let body_copy = if entity.entity_type == ET_BODY {
            self.body_queue_copies
                .get(&entity.number)
                .copied()
                .filter(|copy| usize::from(copy.source_client) == info.client_num)
        } else {
            None
        };

        let e_flags = entity.state.field_i32("eFlags").unwrap_or(0);
        let client_num = entity
            .state
            .field_i32("clientNum")
            .unwrap_or(i32::from(entity.number));
        let runtime = self
            .entities
            .get_mut(&entity.number)
            .ok_or_else(|| "player animation state disappeared".to_owned())?;
        if runtime.model_settled != model_settled
            || !runtime
                .requested_model_name
                .eq_ignore_ascii_case(&info.model_name)
            || !runtime
                .requested_skin_name
                .eq_ignore_ascii_case(&info.skin_name)
        {
            runtime.model_settled = model_settled;
            runtime.requested_model_name.clone_from(&info.model_name);
            runtime.requested_skin_name.clone_from(&info.skin_name);
        }
        let previous_e_flags = runtime.last_e_flags;
        let time_rewound = current_time < runtime.last_angle_time;
        // CG_SetInitialSnapshot calls CG_ResetEntity for every entity, and
        // CG_TransitionEntity does the same whenever interpolation breaks. A
        // newly-created Rust runtime is the initial-snapshot/PVS equivalent.
        let reset_player_entity = recreate
            || openjk_player_entity_needs_reset(
                previous_e_flags,
                runtime.last_client_num,
                e_flags,
                client_num,
                force_reset,
                time_rewound,
            );
        if reset_player_entity {
            // OpenJK CG_TransitionEntity -> CG_ResetEntity ->
            // CG_ResetPlayerEntity when interpolation is broken.  This resets
            // the lerp frames and the persistent torso/legs angle swing state.
            runtime.animation.reset_player_entity();
            // OpenJK CG_ResetPlayerEntity clears/reseeds cent->pe torso/legs
            // swing state, but preserves clientInfo corrTime/lookTime/
            // lastHeadAngles. Only superSmoothTime is reset there.
            runtime
                .player_angles
                .reset_entity_swing_from_angles(entity.angles);
            // OpenJK does not zero cg.frametime on ordinary entity
            // teleports/resets. Only guard an actual local time rewind
            // from producing a negative/huge bridge frametime.
            if time_rewound {
                runtime.last_angle_time = current_time;
            }
        }
        runtime.last_e_flags = e_flags;
        runtime.last_client_num = client_num;

        let weapon = entity.state.field_i32("weapon").unwrap_or(0);
        // WP_SetSaber: "none"/"remove" and the two-handed rules decide which
        // clientInfo_t::saber[] slots actually hold a saber.
        let [has_primary_saber, has_secondary_saber] = self.saber_definitions.equipped_slots(
            [&info.saber_name, &info.saber2_name],
            info.client_num < MAX_CLIENTS,
        );
        // CG_NewClientInfo: when a saber name changes the new saberInfo_t and
        // Ghoul2 hilt instance replace the old ones, and the centity's weapon
        // bookkeeping is cleared ("force a refresh") so the next CG_Player /
        // CG_CheckPlayerG2Weapons recopies BOTH hilts and relights the blades.
        // Without this a second saber selected at runtime (`saber kyle kyle`)
        // stays unattached until the primary is thrown and caught.
        if entity.entity_type != ET_BODY {
            for slot in 0..2 {
                let name = if slot == 0 {
                    &info.saber_name
                } else {
                    &info.saber2_name
                };
                if runtime.saber_names[slot].eq_ignore_ascii_case(name) {
                    continue;
                }
                runtime.saber_names[slot].clone_from(name);
                runtime.ghoul2_weapon = None;
                runtime.cent_weapon = 0;
                // The replacement saberInfo_t starts with zero-length blades
                // that extend gradually toward their desired length.
                for blade in 0..MAX_SABER_BLADES {
                    self.saber_blade_lengths.insert(
                        SaberBladeLengthKey {
                            client_num: info.client_num,
                            saber_num: slot as u8,
                            blade_num: blade as u8,
                        },
                        SaberBladeLengthState::new(
                            DEFAULT_SABER_BLADE_LENGTH_MAX,
                            0.0,
                            current_time,
                        ),
                    );
                }
            }
        }
        if entity.entity_type == ET_BODY {
            // ET_BODY is CG_General in OpenJK. Never derive its held models from
            // currentState.weapon: `ircg` already captured the duplicated G2
            // model slots and CG_BodyQueueCopy's knownWeapon correction.
            runtime.ghoul2_weapon = None;
            runtime.cent_weapon = 0;
            if let Some(copy) = body_copy {
                // CG_BodyQueueCopy operates on an ET_BODY destination. Its
                // low-tier model-1 replacement therefore goes through
                // CG_G2WeaponInstance(body, knownWeapon): WP_SABER is the
                // default g2WeaponInstances saber, not the client's custom
                // primary hilt. Model index 2 is not replaced and can retain
                // the copied custom second saber.
                runtime.attached_weapon = copy.model1_weapon;
                runtime.primary_saber_attached = false;
                runtime.secondary_saber_attached = copy.model2_saber;
            } else {
                runtime.attached_weapon = None;
                runtime.primary_saber_attached = false;
                runtime.secondary_saber_attached = false;
            }
        } else {
            // CG_ResetPlayerEntity performs this remote-saber copy before the
            // ordinary CG_Player weapon update. In particular, if saber 0 is
            // already in flight when a remote player first appears, this copy
            // creates both model index 1 and model index 2; the later flight
            // path removes only index 1, leaving the second saber in hand.
            if reset_player_entity {
                reset_remote_player_saber_attachment(
                    &mut runtime.ghoul2_weapon,
                    &mut runtime.cent_weapon,
                    &mut runtime.attached_weapon,
                    &mut runtime.primary_saber_attached,
                    &mut runtime.secondary_saber_attached,
                    weapon,
                    i32::from(entity.number) != self.viewer_client,
                    has_primary_saber,
                    has_secondary_saber,
                );
            }

            let dead = e_flags & EF_DEAD != 0;
            let instance = if weapon == WP_SABER && has_primary_saber {
                Some(WP_SABER)
            } else {
                weapon_world_model(weapon).map(|_| weapon)
            };
            update_weapon_attachment(
                &mut runtime.ghoul2_weapon,
                &mut runtime.cent_weapon,
                &mut runtime.attached_weapon,
                &mut runtime.primary_saber_attached,
                &mut runtime.secondary_saber_attached,
                weapon,
                instance,
                entity.state.field_i32("saberInFlight").unwrap_or(0) != 0,
                dead,
                info.team == TEAM_SPECTATOR && entity.entity_type == ET_PLAYER,
                has_primary_saber,
                has_secondary_saber,
            );

            // CG_Player's saber presentation removes model index 1 while the
            // primary saber is in flight and copies it back when held again.
            // This is model-slot state, separate from whether blades are lit.
            if weapon == WP_SABER && runtime.cent_weapon == WP_SABER && has_primary_saber {
                if entity.state.field_i32("saberInFlight").unwrap_or(0) != 0 {
                    runtime.primary_saber_attached = false;
                } else {
                    runtime.attached_weapon = None;
                    runtime.primary_saber_attached = true;
                }
            }
        }
        // Preserve model slot 1 before applying CG_General's source-instance
        // mutations. The detached Ghoul2 copy is made after slot 2/3 are
        // removed but before slot 1 is removed for weapon-arm/waist severs.
        let detached_model1_weapon = runtime.attached_weapon;
        let detached_model1_primary_saber = runtime.primary_saber_attached;
        let mut attached_weapon = runtime.attached_weapon;
        let mut attached_sabers = [
            runtime.primary_saber_attached,
            runtime.secondary_saber_attached,
        ];
        if let Some(parts) = self.dismembered.get(&entity.number) {
            // OpenJK removes Ghoul2 model index 2 from the source before every
            // limb duplicate (and model index 3/jetpack as well). The duplicate
            // therefore never contains the second saber.
            attached_sabers[1] = false;
            // After duplicating, model index 1 is removed from the source only
            // when its weapon-holding subtree was actually severed.
            if parts.iter().copied().any(DismemberPart::removes_weapon) {
                attached_weapon = None;
                attached_sabers[0] = false;
            }
        }

        // OpenJK renders ET_BODY through CG_General using the Ghoul2 instance
        // copied at respawn. Our renderer still owns the equivalent corpse pose
        // and Rapier handoff here; once ragging, preserve the existing yaw-only
        // corpse orientation used by this port.
        runtime.animation.animator.clear_bone_angle_overrides();
        // Match OpenJK CG_G2PlayerAngles/CG_G2Animated: dead entities and
        // explicit EF_RAG entities enter the ragdoll seam regardless of whether
        // they have already been copied into the ET_BODY queue. This is crucial
        // for NPCs, which can remain ET_NPC while dead.
        let corpse_ragdoll_requested =
            entity.entity_type == ET_BODY || e_flags & (EF_DEAD | EF_RAG) != 0;
        let force_grip_ragdoll_requested =
            !corpse_ragdoll_requested && self.force_gripped_entities.contains(&entity.number);
        let impulse_ragdoll = (!corpse_ragdoll_requested && !force_grip_ragdoll_requested)
            .then(|| self.active_impulse_ragdolls.get(&entity.number).copied())
            .flatten()
            .filter(|impulse| current_time < impulse.expires_at);
        let impulse_ragdoll_requested = impulse_ragdoll.is_some();
        let ragdoll_requested =
            corpse_ragdoll_requested || force_grip_ragdoll_requested || impulse_ragdoll_requested;
        // OpenJK's corpse/Broadsword path forces yaw-only orientation. A living
        // Force-grip handoff must instead sample the exact normal player pose on
        // its first frame, then Rapier freezes that skinning pose internally.
        let rapier_corpse = corpse_ragdoll_requested && self.ragdolls.active();
        let (axis, torso_angles) = if rapier_corpse {
            runtime.last_angle_time = current_time;
            (angles_to_axis([0.0, entity.angles[1], 0.0]), [0.0; 3])
        } else {
            // CG_Player calls CG_G2PlayerAngles (-> BG_G2PlayerAngles) before
            // CG_PlayerAnimation. The Motion bolt and the client's installed
            // legs/torso animation IDs queried here are therefore still last
            // frame's values: BG_G2ClientSpineAngles' entity-vs-client ID guard
            // depends on seeing that one-frame lag to detect an animation
            // change. Evaluate them ahead of `runtime.animation.update` below
            // to keep that ordering, rather than after it.
            let motion_index = Ghoul2Animator::bone_index(&model.gla, "Motion")
                .ok_or_else(|| format!("{} has no Ghoul2 Motion bone", info.model_qpath()))?;
            let pose_started = Instant::now();
            // OpenJK's CBoneCache::Eval is lazy: BG_G2PlayerAngles only needs the
            // pre-angle Motion bolt, so evaluate Motion + its ancestors here rather
            // than materializing all humanoid bones a first time.
            let motion_pose = runtime.animation.animator.evaluate_bone_openjk_root(
                &model.gla,
                current_time,
                motion_index,
            )?;
            record_motion_pose_eval(&mut self.perf, pose_started);
            let motion_bolt =
                multiply_3x4(&motion_pose, &model.gla.skeleton[motion_index].base_pose);
            let motion_matrix = flatten_matrix3x4(&motion_bolt);

            let mut look_angles = entity.angles;
            if entity.state.field_i32("hasLookTarget").unwrap_or(0) != 0 {
                if let Some(target_origin) = look_target_origin {
                    look_angles = openjk_vectoangles([
                        target_origin[0] - entity.origin[0],
                        target_origin[1] - entity.origin[1],
                        target_origin[2] - entity.origin[2],
                    ]);
                    runtime.player_angles.look_time = current_time.saturating_add(1000);
                }
            }
            // Exact CG_Player behavior immediately before BG_G2PlayerAngles.
            look_angles[0] = 0.0;

            let frametime = current_time.saturating_sub(runtime.last_angle_time);
            let (ci_legs, ci_torso) = runtime.animation.client_animation_ids();
            let model_scale_int = entity.state.field_i32("iModelScale").unwrap_or(0);
            let model_scale = if model_scale_int != 0 {
                let scale = model_scale_int as f32 / 100.0;
                [scale, scale, scale]
            } else {
                [0.0; 3]
            };
            let angle_entity = player_angle_entity(entity);
            let angle_render_origin = ghoul2_render_origin(entity);
            let angle_result = bg_g2_player_angles(
                &angle_entity,
                current_time,
                angle_render_origin,
                entity.angles,
                frametime,
                model_scale,
                ci_legs,
                ci_torso,
                look_angles,
                Some(&motion_matrix),
                &mut runtime.player_angles,
            )?;
            runtime.last_angle_time = current_time;
            let torso_angles = [
                runtime.player_angles.torso_pitch_angle,
                runtime.player_angles.torso_yaw_angle,
                0.0,
            ];

            for command in angle_result.commands() {
                if command.flags & BONE_ANGLES_POSTMULT == 0 {
                    return Err(format!(
                        "OpenJK BG_G2PlayerAngles emitted unsupported bone-angle flags {:#x} for {}",
                        command.flags,
                        command.bone_name()
                    ));
                }
                runtime.animation.animator.set_bone_angles_postmult(
                    &model.gla,
                    command.bone_name(),
                    command.angles,
                    command.up,
                    command.right,
                    command.forward,
                )?;
            }
            (angle_result.axis(), torso_angles)
        };

        // CG_PlayerAnimation runs after CG_G2PlayerAngles: advance the legs/
        // torso lerp frames, client animation IDs, and Motion bone schedule
        // now that the angle calculation above has consumed last frame's
        // values.
        runtime.animation.update(
            &entity.state,
            &self.animations,
            &model.gla,
            info,
            &self.saber_scales,
            current_time,
            true,
        )?;

        let anim_debug = ViewerAnimDebug {
            legs_input: entity.state.field_i32("legsAnim").unwrap_or(0),
            torso_input: entity.state.field_i32("torsoAnim").unwrap_or(0),
            legs_anim: runtime.animation.legs.animation_number,
            legs_frame: runtime.animation.legs.frame,
            legs_old_frame: runtime.animation.legs.old_frame,
            legs_backlerp: runtime.animation.legs.backlerp,
            torso_anim: runtime.animation.torso.animation_number,
            torso_frame: runtime.animation.torso.frame,
            torso_old_frame: runtime.animation.torso.old_frame,
            torso_backlerp: runtime.animation.torso.backlerp,
            ..ViewerAnimDebug::default()
        };

        // CG_TriggerAnimSounds: the footsteps the legs bone just stepped onto.
        let mut footsteps = Vec::new();
        if self.footstep_stages.any()
            && entity.entity_type != ET_BODY
            && footsteps::npc_class_steps(entity.state.field_i32("NPC_class").unwrap_or(0))
        {
            if let Some(step) = runtime.animation.legs_frame_step() {
                let mut seed = self.footstep_rng;
                footsteps = footsteps::footsteps_between(
                    &self.anim_events.legs,
                    &self.animations,
                    step.anim,
                    step.old_frame,
                    step.frame,
                    || {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        ((seed >> 16) % 100) as i32
                    },
                );
                self.footstep_rng = seed;
            }
        }

        let cull_origin = ghoul2_render_origin(entity);
        let mut raster_visible = submit_geometry;

        // Keep the cgame-side player-angle/animation state advancing exactly
        // as it does for a visible entity, then mirror OpenJK's renderer-side
        // whole-Ghoul2 sphere rejection before the expensive final full pose.
        // Culled models therefore pay only the lazy Motion-bone chain above.
        let body_lod = if submit_geometry {
            if let Some(view) = view {
                if self.early_frustum_cull && entity.entity_type != ET_BODY {
                    self.perf.frustum_tests = self.perf.frustum_tests.saturating_add(1);
                    let radius = ghoul2_entity_radius(entity) * ghoul2_model_scale(entity);
                    if view.sphere_outside(cull_origin, radius) {
                        self.perf.frustum_culled = self.perf.frustum_culled.saturating_add(1);
                        if !self.rt_shadow_casters_enabled {
                            // Out of view is not out of earshot: the pose is not built,
                            // so the feet are taken to be under the entity.
                            if let (false, Some(world)) =
                                (footsteps.is_empty(), self.collision_world.as_mut())
                            {
                                footsteps::push_footsteps(
                                    world,
                                    &mut self.footsteps,
                                    entity.number,
                                    runtime.player_angles.legs_yaw_angle,
                                    &footsteps,
                                    |_| footsteps::unposed_foot(entity.origin),
                                );
                            }
                            return Ok(Vec::new());
                        }
                        // Camera visibility is not shadow visibility: a caster
                        // just outside the viewport may still shadow visible BSP.
                        raster_visible = false;
                    }
                }
                let lod = ghoul2_lod_for_view(
                    view,
                    entity,
                    cull_origin,
                    model.glm.lods.len(),
                    self.lod_bias,
                    self.lod_scale,
                );
                let bucket = lod.min(self.perf.lod_counts.len() - 1);
                self.perf.lod_counts[bucket] = self.perf.lod_counts[bucket].saturating_add(1);
                lod
            } else {
                self.perf.lod_counts[0] = self.perf.lod_counts[0].saturating_add(1);
                0
            }
        } else {
            0
        };

        let pose_started = Instant::now();
        let mut pose = runtime
            .animation
            .evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);

        let (render_axis, render_origin) = ghoul2_render_transform(entity, axis);
        if ragdoll_requested {
            self.ragdolls.apply_to_pose(
                entity,
                &model.key,
                &model.gla,
                &mut pose,
                axis,
                render_origin,
                ghoul2_model_scale(entity),
                if corpse_ragdoll_requested {
                    RagdollMode::Corpse
                } else if force_grip_ragdoll_requested {
                    RagdollMode::ForceGrip
                } else {
                    RagdollMode::Impulse
                },
                impulse_ragdoll.map(|impulse| impulse.seed_velocity),
                current_time,
            )?;
        } else {
            self.ragdolls
                .apply_living_recovery(entity.number, &model.key, &mut pose, current_time);
        }

        // jaPRO `r_ghoul2animsmooth`/`CBoneCache::SmoothLow`: filter the final
        // composed pose against last frame's filtered pose before it reaches
        // GPU skinning, bolt/attachment queries, and the draw pose below, so
        // all three stay consistent with each other. Ragdoll is its own
        // physically continuous pose; skip and reseed rather than blending an
        // animated-pose history across that discontinuity.
        if self.ghoul2_anim_smooth > 0.0 && self.ghoul2_anim_smooth < 1.0 && !ragdoll_requested {
            // A frustum-culled frame returns before this point without
            // touching `bone_smooth_time`, so a long-culled entity reappearing
            // is still caught here even though nothing reset the history above.
            const BONE_SMOOTH_MAX_GAP_MS: i32 = 250;
            if let Some(history) = runtime.bone_smooth_history.as_ref().filter(|history| {
                history.len() == pose.len()
                    && current_time.saturating_sub(runtime.bone_smooth_time)
                        <= BONE_SMOOTH_MAX_GAP_MS
            }) {
                pose = smooth_ghoul2_pose(&model.gla, history, &pose, self.ghoul2_anim_smooth);
            }
            runtime.bone_smooth_history = Some(pose.clone());
            runtime.bone_smooth_time = current_time;
        } else {
            runtime.bone_smooth_history = None;
        }

        if entity.entity_type == ET_PLAYER && i32::from(entity.number) == self.viewer_client {
            self.viewer_foot_bolts =
                foot_bolts_timed(&mut self.perf, &model, &pose, ghoul2_model_scale(entity));
            let key_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                model.key.hash(&mut hasher);
                hasher.finish() as u32
            };
            let mut head_world = None;
            self.viewer_anim_debug = Some(ViewerAnimDebug {
                pose_time: current_time,
                origin: entity.origin,
                angles: entity.angles,
                feet: self.viewer_foot_bolts.unwrap_or_default(),
                bones: {
                    let scale = ghoul2_model_scale(entity);
                    let mut bone = |name| {
                        let matrix = model_bolt_matrix_timed(
                            &mut self.perf,
                            &model.glm,
                            &model.gla,
                            &pose,
                            name,
                        )
                        .ok()
                        .flatten();
                        let point =
                            matrix.map(|m| [m[0][3] * scale, m[1][3] * scale, m[2][3] * scale]);
                        if name == "*head_eyes" {
                            head_world = point.map(|point| {
                                transform_jka_model_point(point, render_axis, render_origin)
                            });
                        }
                        point.unwrap_or([0.0; 3])
                    };
                    [
                        bone("*r_hand"),
                        bone("*l_hand"),
                        bone("*head_eyes"),
                        bone("*chestg"),
                    ]
                },
                head_world,
                render_origin,
                render_yaw: render_axis[0][1].atan2(render_axis[0][0]).to_degrees(),
                model_hash: key_hash,
                model_settled,
                ..anim_debug
            });
        }
        if let (false, Some(world)) = (footsteps.is_empty(), self.collision_world.as_mut()) {
            // CG_PlayerFootsteps: the foot bolt in the legs' own frame.
            let perf = &mut self.perf;
            footsteps::push_footsteps(
                world,
                &mut self.footsteps,
                entity.number,
                runtime.player_angles.legs_yaw_angle,
                &footsteps,
                |foot| {
                    let bolt = if foot.right_foot() {
                        "*r_leg_foot"
                    } else {
                        "*l_leg_foot"
                    };
                    model_bolt_origin_timed(perf, &model, &pose, render_axis, render_origin, bolt)
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| footsteps::unposed_foot(entity.origin))
                },
            );
        }
        self.queue_force_fx(
            entity,
            &model,
            &pose,
            render_axis,
            render_origin,
            torso_angles,
            submit_geometry,
            current_time,
        )?;

        if !submit_geometry && !self.rt_shadow_casters_enabled {
            // OpenJK still runs CG_Player for the local player in first person
            // and marks the refEntity RF_THIRD_PERSON.  Keep all animation and
            // Ghoul2 state advancing, but suppress geometry submission here.
            self.report_player_status(
                entity.number,
                format!(
                    "clientNum={} requested={}/{} resolved={} submitted=0 reason=first-person-body",
                    info.client_num, info.model_name, info.skin_name, model.key,
                ),
            );
            return Ok(Vec::new());
        }

        // `axis` is the `legs` matrix returned by the actual OpenJK
        // BG_G2PlayerAngles call, matching CG_Player's entity axis.
        // CG_Player: legs.shaderRGBA = cent->currentState.customRGBA (`char_color_*`).
        let look = self.look;
        let (body_rgba, body_rgb, body_custom) = match look.ghost {
            crate::japro_cg::Ghost::None => {
                let mut rgb = info.team_color_override.map_or_else(
                    || player_entity_rgb(entity),
                    |rgb| rgb.map(|channel| f32::from(channel) / 255.0),
                );
                if look.duel_bubble {
                    // Duelists seen from outside: shaderRGBA = 50 with RF_RGB_TINT.
                    rgb = [50.0 / 255.0; 3];
                } else if look.dim {
                    // Bystanders of your duel are drawn at a fifth of the light.
                    rgb = rgb.map(|channel| channel / 5.0);
                }
                ([1.0, 1.0, 1.0, entity_alpha], rgb, None)
            }
            ghost => {
                let mut rgba = crate::japro_cg::ghost_color(ghost);
                rgba[3] = entity_alpha;
                (rgba, [1.0; 3], Some(self.ghost_material(ghost)))
            }
        };
        let dismember_selection =
            source_dismember_surface_set(&model, self.dismembered.get(&entity.number));
        if dismember_selection.is_some() {
            self.dismember_source_snaps.insert(
                entity.number,
                DismemberSourceSnap {
                    model: Arc::clone(&model),
                    pose: pose.clone(),
                    axis: render_axis,
                    origin: render_origin,
                    model_scale: ghoul2_model_scale(entity),
                    body_rgba,
                    body_rgb,
                    model1_weapon: detached_model1_weapon,
                    model1_primary_saber: detached_model1_primary_saber,
                    client_info: info.clone(),
                },
            );
        }

        // EternalJK FPLS mode 3 keeps the local Ghoul2 instance alive so its
        // attached saber hilt/blade presentation still works, but turns off the
        // player-body roots with G2SURFACEFLAG_NODESCENDANTS. The local Ghoul2
        // saber remains attached, while EternalJK still runs CG_AddViewWeapon
        // afterwards for the weapon's ordinary first-person viewModel.
        let fpls_surfaces;
        let body_surfaces = if first_person_saber {
            fpls_surfaces = model
                .surfaces
                .iter()
                .filter(|surface| !fpls_mode3_surface_hidden(&model.glm, surface.surface_index))
                .cloned()
                .collect::<Vec<_>>();
            fpls_surfaces.as_slice()
        } else {
            model.surfaces.as_slice()
        };
        let mut draws = self.render_glm_surfaces_tinted_selected(
            entity.number,
            &model.key,
            &model.glm,
            &model.gla,
            body_surfaces,
            model.jiggle.as_deref(),
            &pose,
            body_lod,
            render_axis,
            render_origin,
            body_rgba,
            body_rgb,
            body_custom,
            true,
            dismember_selection.as_ref(),
        )?;

        // TaystJK CG_Player model index 3: when EF_JETPACK is present, bolt
        // models/weapons2/jetpack/model.glm to the player's *chestg bolt. The
        // same child pose owns torso_ljet/torso_rjet, which are also the
        // authoritative origins/directions for the stock Boba exhaust EFX.
        if e_flags & EF_JETPACK != 0 && e_flags & EF_DEAD == 0 {
            if let Err(error) = self.append_jetpack(
                &mut draws,
                entity,
                &model,
                &pose,
                render_axis,
                render_origin,
                current_time,
                entity_alpha,
            ) {
                self.report_saber_warning_once(entity.number, &format!("jetpack: {error}"));
            }
        }

        self.report_player_status(
            entity.number,
            format!(
                "clientNum={} requested={}/{} resolved={} bodySurfaces={} submitted={}",
                info.client_num,
                info.model_name,
                info.skin_name,
                model.key,
                draws.len(),
                usize::from(!draws.is_empty()),
            ),
        );
        if self.gore_limit > 0 {
            let snaps = draws
                .iter()
                .filter_map(|surface| {
                    surface
                        .ghoul2_gpu
                        .as_ref()
                        .map(|skin| crate::cgame::player_gore::GpuSnap {
                            mesh_key: Arc::clone(&skin.mesh_key),
                            vertices: Arc::clone(&skin.vertices),
                            indices: Arc::clone(&skin.indices),
                            bones: Arc::clone(&skin.bones),
                            jiggle_offsets: skin.jiggle_offsets,
                            axis: skin.axis,
                            origin: skin.origin,
                        })
                })
                .collect();
            self.last_gpu.insert(entity.number, snaps);
            self.append_gore(entity.number, current_time, &mut draws);
        }
        let shells = self.force_shells(entity, current_time);
        if !shells.is_empty() {
            let body = draws.clone();
            let seconds = self.stage_time_ms as f32 * 0.001;
            for (shader, rgba, force_alpha_blend) in shells {
                let (texture, mut alpha_mode) = self.custom_shader_material(shader);
                if force_alpha_blend {
                    // RF_FORCE_ENT_ALPHA: the team-power shell draws with
                    // standard alpha blending regardless of its own shader's
                    // (additive) blendFunc.
                    alpha_mode = DynamicModelAlphaMode::BlendUnlit;
                }
                let uv_xform = self.custom_shader_uv_xform(shader, seconds);
                let bulge_height = self.custom_shader_bulge_height(shader, seconds);
                let env_map = self.custom_shader_env_map(shader);
                let overlays =
                    self.custom_shader_overlay_stages(shader, seconds, force_alpha_blend);
                for surface in &body {
                    let mut shell = Self::clone_surface_with_material_xform(
                        surface,
                        texture.clone(),
                        alpha_mode,
                        rgba,
                        Some(uv_xform),
                        Some(bulge_height),
                        Some(env_map),
                    );
                    // rgbGen identity/const customShader shells are flat-colored,
                    // scene-light-independent overlays (RF_RGB_TINT forces the
                    // color outright) and never their own shadow casters.
                    shell.lighting_origin = None;
                    shell.rt_rigid = None;
                    shell.rt_skinned_key = None;
                    draws.push(shell);
                    for (overlay_texture, overlay_mode, overlay_uv) in &overlays {
                        let mut overlay = Self::clone_surface_with_material_xform(
                            surface,
                            overlay_texture.clone(),
                            *overlay_mode,
                            rgba,
                            Some(*overlay_uv),
                            Some(bulge_height),
                            Some(env_map),
                        );
                        overlay.lighting_origin = None;
                        overlay.rt_rigid = None;
                        overlay.rt_skinned_key = None;
                        draws.push(overlay);
                    }
                }
            }
        }
        if submit_geometry {
            self.queue_cosmetics(
                entity,
                info,
                &model,
                &pose,
                render_axis,
                render_origin,
                look.ghost,
                entity_alpha,
            );
        }
        if attached_sabers[0] || attached_sabers[1] {
            if let Err(error) = self.append_player_sabers(
                &mut draws,
                entity,
                info,
                &model,
                &pose,
                render_axis,
                render_origin,
                current_time,
                entity_alpha,
                attached_sabers,
                entity.entity_type != ET_BODY,
            ) {
                self.report_saber_warning_once(entity.number, &error);
            }
        }
        if let Some(weapon) = attached_weapon {
            if let Err(error) = self.append_held_weapon(
                &mut draws,
                entity,
                weapon,
                &model,
                &pose,
                render_axis,
                render_origin,
                current_time,
                entity_alpha,
            ) {
                self.report_saber_warning_once(
                    entity.number,
                    &format!("held weapon {weapon}: {error}"),
                );
            }
        }
        if !raster_visible {
            for surface in &mut draws {
                surface.raster_visible = false;
            }
        }
        Ok(draws)
    }
}
