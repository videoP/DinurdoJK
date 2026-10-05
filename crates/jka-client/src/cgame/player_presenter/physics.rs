//! Physics.
use crate::cgame::player_presenter::{
    angles_to_axis, force_grip_target_alive, force_grip_target_still_valid,
    impulse_weapon_is_explosive, infer_force_grip_target, presented_entity_in_knockdown,
    presented_entity_velocity, AnimationSet, Arc, ClothConfig, CollisionWorld, DynamicModelSurface,
    DynamicWireframeClass, EntityEvent, FxGpuSprites, Ghoul2PerfStats, Ghoul2SkinningMode, HashMap,
    HashSet, PhysicsMapMesh, PlayerPresenter, PresentationEvent, PresentedEntity, RagdollConfig,
    Vec3, EF_DEAD, ET_NPC, ET_PLAYER, FP_GRIP,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::cgame::player_presenter) enum ImpulseRagdollSource {
    Weapon,
    Explosion,
    Force,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::cgame::player_presenter) struct PendingImpulseRagdoll {
    pub(in crate::cgame::player_presenter) source: ImpulseRagdollSource,
    pub(in crate::cgame::player_presenter) expires_at: i32,
    /// World-space point the impulse should move away from (weapon/explosion),
    /// or toward for Force Pull when `toward_origin` is true.
    pub(in crate::cgame::player_presenter) origin: [f32; 3],
    pub(in crate::cgame::player_presenter) toward_origin: bool,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::cgame::player_presenter) struct ActiveImpulseRagdoll {
    pub(in crate::cgame::player_presenter) source: ImpulseRagdollSource,
    pub(in crate::cgame::player_presenter) expires_at: i32,
    /// Pre-impact trajectory velocity. Free limbs inherit this while the
    /// thoracic kinematic anchor follows the new authoritative trajectory.
    pub(in crate::cgame::player_presenter) seed_velocity: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub(in crate::cgame::player_presenter) struct ExplosionImpulsePulse {
    pub(in crate::cgame::player_presenter) origin: [f32; 3],
    pub(in crate::cgame::player_presenter) expires_at: i32,
}

pub(in crate::cgame::player_presenter) const IMPULSE_RAGDOLL_MS: i32 = 600;

pub(in crate::cgame::player_presenter) const IMPULSE_CANDIDATE_MS: i32 = 250;

pub(in crate::cgame::player_presenter) const IMPULSE_DELTA_MIN: f32 = 48.0;

pub(in crate::cgame::player_presenter) const EXPLOSION_CANDIDATE_RADIUS: f32 = 512.0;

pub(in crate::cgame::player_presenter) const FORCE_CANDIDATE_RADIUS: f32 = 1024.0;

pub(in crate::cgame::player_presenter) const ANIM_TOGGLEBIT: i32 = 2048;

impl PlayerPresenter {
    pub(in crate::cgame::player_presenter) fn observe_impulse_event(
        &mut self,
        event: &PresentationEvent,
    ) {
        const EF_ALT_FIRING: i32 = 1 << 10;
        let state = &event.state;
        let weapon = state.field_i32("weapon").unwrap_or(0);
        let alt = state.field_i32("eFlags").unwrap_or(0) & EF_ALT_FIRING != 0;
        let explosive = impulse_weapon_is_explosive(weapon, alt);

        match event.event {
            EntityEvent::EV_SABER_HIT if self.ragdolls.weapon_impulses_enabled() => {
                if let Ok(target) = u16::try_from(state.field_i32("otherEntityNum").unwrap_or(-1)) {
                    self.queue_impulse_candidate(
                        target,
                        ImpulseRagdollSource::Weapon,
                        event.position,
                        false,
                        event.server_time,
                    );
                }
            }
            EntityEvent::EV_MISSILE_HIT => {
                let source = if explosive {
                    ImpulseRagdollSource::Explosion
                } else {
                    ImpulseRagdollSource::Weapon
                };
                let enabled = match source {
                    ImpulseRagdollSource::Weapon => self.ragdolls.weapon_impulses_enabled(),
                    ImpulseRagdollSource::Explosion => self.ragdolls.explosion_impulses_enabled(),
                    ImpulseRagdollSource::Force => false,
                };
                if enabled {
                    if let Ok(target) =
                        u16::try_from(state.field_i32("otherEntityNum").unwrap_or(-1))
                    {
                        self.queue_impulse_candidate(
                            target,
                            source,
                            event.position,
                            false,
                            event.server_time,
                        );
                    }
                }
                if explosive && self.ragdolls.explosion_impulses_enabled() {
                    self.explosion_impulse_pulses.push(ExplosionImpulsePulse {
                        origin: event.position,
                        expires_at: event.server_time.saturating_add(IMPULSE_CANDIDATE_MS),
                    });
                }
            }
            EntityEvent::EV_MISSILE_MISS | EntityEvent::EV_MISSILE_MISS_METAL
                if explosive && self.ragdolls.explosion_impulses_enabled() =>
            {
                self.explosion_impulse_pulses.push(ExplosionImpulsePulse {
                    origin: event.position,
                    expires_at: event.server_time.saturating_add(IMPULSE_CANDIDATE_MS),
                });
            }
            _ => {}
        }
    }

    pub(in crate::cgame::player_presenter) fn queue_impulse_candidate(
        &mut self,
        target: u16,
        source: ImpulseRagdollSource,
        origin: [f32; 3],
        toward_origin: bool,
        current_time: i32,
    ) {
        self.pending_impulse_ragdolls.insert(
            target,
            PendingImpulseRagdoll {
                source,
                expires_at: current_time.saturating_add(IMPULSE_CANDIDATE_MS),
                origin,
                toward_origin,
            },
        );
    }

    pub fn set_gore_limit(&mut self, limit: usize) {
        self.gore_limit = limit.min(64);
        if self.gore_limit == 0 {
            self.gore.clear();
            self.last_gpu.clear();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_jiggle_config(
        &mut self,
        enabled: bool,
        hz: u32,
        max_substeps: u32,
        solver: u8,
        overall_strength: f32,
        breast_strength: f32,
        glute_strength: f32,
        stiffness_scale: f32,
        damping_scale: f32,
        glute_lift: f32,
        jp_stiffness: f32,
        jp_drag: f32,
        jp_air_drag: f32,
        jp_stretch: f32,
        jp_soften: f32,
        jp_gravity: f32,
    ) {
        self.jiggle.set_config(
            enabled,
            hz,
            max_substeps,
            crate::cgame::jiggle::JiggleTuning {
                solver: crate::cgame::jiggle::JiggleSolver::from_u8(solver),
                overall_strength,
                breast_strength,
                glute_strength,
                stiffness_scale,
                damping_scale,
                glute_lift,
                jp_stiffness,
                jp_drag,
                jp_air_drag,
                jp_stretch,
                jp_soften,
                jp_gravity,
            },
        );
    }

    pub(crate) fn set_cloth_config(&mut self, config: ClothConfig) {
        self.cloth.set_config(config);
    }

    pub(crate) fn set_cloth_wind(&mut self, wind: Option<crate::ocean::OceanWind>) {
        self.cloth_weather_wind = wind;
    }

    pub fn set_ragdoll_config(&mut self, config: RagdollConfig) {
        let previous_dismemberment = self.ragdolls.dismemberment_level();
        let dismemberment = config.dismemberment;
        self.ragdolls.set_config(config);
        if previous_dismemberment != dismemberment {
            // Surface mutations are presentation state. Rebuild them from the
            // currently transmitted G2_MODEL_PART entities on the next frame.
            self.dismembered.clear();
            self.dismember_source_snaps.clear();
            self.detached_limb_visuals.clear();
        }
    }

    pub fn set_physics_map_mesh(&mut self, mesh: &PhysicsMapMesh) -> Result<(), String> {
        self.ragdolls.set_map_mesh(mesh)
    }

    /// Resolve which living entity is currently being Force-gripped. Vanilla
    /// multiplayer does not send forceGripEntityNum in entityState, so remote
    /// targets are reconstructed from the same information the original server
    /// uses for acquisition: active FP_GRIP, full view angles, and a 256-unit
    /// forward trace. Existing targets are preserved while they stay valid so
    /// level-3 grip can swing the victim without target jitter.
    ///
    /// `local_force_gripped` comes from cg.snap/predicted ps.fd.forceGripCripple
    /// and is exact for the local/followed player.
    pub fn prepare_force_grip_targets(
        &mut self,
        entities: &[PresentedEntity],
        followed: Option<&PresentedEntity>,
        local_force_gripped: bool,
        current_time: i32,
    ) {
        let mut all = entities
            .iter()
            .filter(|entity| entity.entity_type == ET_PLAYER || entity.entity_type == ET_NPC)
            .collect::<Vec<_>>();
        if let Some(followed) = followed {
            if !all.iter().any(|entity| entity.number == followed.number) {
                all.push(followed);
            }
        }

        let by_number = all
            .iter()
            .map(|entity| (entity.number, *entity))
            .collect::<HashMap<_, _>>();
        let active_grippers = all
            .iter()
            .copied()
            .filter(|entity| {
                entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0
                    && entity.state.field_i32("forcePowersActive").unwrap_or(0) & (1 << FP_GRIP)
                        != 0
            })
            .collect::<Vec<_>>();

        let mut next_targets = HashMap::new();
        let mut gripped = HashSet::new();

        for gripper in active_grippers {
            let preserved = self
                .force_grip_targets
                .get(&gripper.number)
                .and_then(|target_num| by_number.get(target_num).copied())
                .filter(|target| force_grip_target_still_valid(gripper, target));
            let target = preserved.or_else(|| infer_force_grip_target(gripper, &all));
            if let Some(target) = target {
                next_targets.insert(gripper.number, target.number);
                gripped.insert(target.number);
            }
        }

        if local_force_gripped {
            if let Some(followed) = followed {
                gripped.insert(followed.number);
            }
        }

        let released = self
            .force_gripped_entities
            .difference(&gripped)
            .copied()
            .collect::<Vec<_>>();
        for entity in &released {
            self.ragdolls.begin_living_recovery(*entity, current_time);
        }

        if self.ragdolls.debug_enabled() {
            for entity in gripped.difference(&self.force_gripped_entities) {
                let gripper = next_targets
                    .iter()
                    .find_map(|(gripper, target)| (*target == *entity).then_some(*gripper));
                println!(
                    "RAPIER FORCE GRIP START: victim={} gripper={}",
                    entity,
                    gripper
                        .map_or_else(|| "local-playerstate".to_owned(), |value| value.to_string()),
                );
            }
            for entity in &released {
                println!("RAPIER FORCE GRIP END: victim={entity}");
            }
        }
        self.force_grip_targets = next_targets;
        self.force_gripped_entities = gripped;
    }

    /// Resolve presentation-only impulse ragdolls from authoritative gameplay
    /// evidence. The local/followed player can use PMF_TIME_KNOCKBACK exactly;
    /// remote entityState cannot, so remote activation requires both a known
    /// impact/Force candidate and a matching server trajectory-velocity change.
    pub fn prepare_impulse_ragdolls(
        &mut self,
        entities: &[PresentedEntity],
        followed: Option<&PresentedEntity>,
        local_knockback: bool,
        current_time: i32,
    ) {
        let mut all = entities
            .iter()
            .filter(|entity| entity.entity_type == ET_PLAYER || entity.entity_type == ET_NPC)
            .collect::<Vec<_>>();
        if let Some(followed) = followed {
            if !all.iter().any(|entity| entity.number == followed.number) {
                all.push(followed);
            }
        }
        let live = all
            .iter()
            .map(|entity| entity.number)
            .collect::<HashSet<_>>();
        let followed_num = followed.map(|entity| entity.number);

        if !self.ragdolls.active() {
            self.pending_impulse_ragdolls.clear();
            self.active_impulse_ragdolls.clear();
            self.explosion_impulse_pulses.clear();
            self.previous_impulse_velocity.clear();
            self.previous_impulse_velocity.extend(
                all.iter()
                    .map(|entity| (entity.number, presented_entity_velocity(entity))),
            );
            self.force_gesture_anim.clear();
            self.force_gesture_anim.extend(all.iter().map(|entity| {
                (
                    entity.number,
                    entity.state.field_i32("torsoAnim").unwrap_or(0),
                )
            }));
            return;
        }

        self.pending_impulse_ragdolls.retain(|entity, candidate| {
            live.contains(entity) && current_time <= candidate.expires_at
        });
        self.explosion_impulse_pulses
            .retain(|pulse| current_time <= pulse.expires_at);
        self.previous_impulse_velocity
            .retain(|entity, _| live.contains(entity));
        self.force_gesture_anim
            .retain(|entity, _| live.contains(entity));

        // Explosion events do not enumerate splash victims. Treat nearby living
        // players as candidates only; a trajectory discontinuity is still
        // required below before a ragdoll is activated.
        if self.ragdolls.explosion_impulses_enabled() {
            let pulses = self.explosion_impulse_pulses.clone();
            let mut candidates = Vec::new();
            for pulse in pulses {
                for target in &all {
                    if !force_grip_target_alive(target) {
                        continue;
                    }
                    let delta = Vec3::from_array(target.origin) - Vec3::from_array(pulse.origin);
                    if delta.length_squared() <= EXPLOSION_CANDIDATE_RADIUS.powi(2) {
                        candidates.push((target.number, pulse.origin));
                    }
                }
            }
            for (target, origin) in candidates {
                self.queue_impulse_candidate(
                    target,
                    ImpulseRagdollSource::Explosion,
                    origin,
                    false,
                    current_time,
                );
            }
        }

        // ForceThrow itself has no victim event in protocol 26. OpenJK does,
        // however, drive BOTH_FORCEPUSH/BOTH_FORCEPULL through the networked
        // torsoAnim. Use the animation *transition* to open a short candidate
        // window, then require the victim's authoritative velocity to change in
        // the matching push/pull direction.
        if self.ragdolls.force_impulses_enabled() {
            let mut gestures = Vec::new();
            for source in &all {
                let raw = source.state.field_i32("torsoAnim").unwrap_or(0);
                let animation = AnimationSet::name(raw & !ANIM_TOGGLEBIT);
                let previous = self.force_gesture_anim.insert(source.number, raw);
                if previous == Some(raw) {
                    continue;
                }
                let toward_source = match animation {
                    Some("BOTH_FORCEPUSH") => false,
                    Some("BOTH_FORCEPULL") => true,
                    _ => continue,
                };
                gestures.push((source.number, source.origin, source.angles, toward_source));
            }

            let mut candidates = Vec::new();
            for (source_num, source_origin, source_angles, toward_source) in gestures {
                let forward =
                    Vec3::from_array(angles_to_axis(source_angles)[0]).normalize_or_zero();
                for target in &all {
                    if target.number == source_num || !force_grip_target_alive(target) {
                        continue;
                    }
                    let offset = Vec3::from_array(target.origin) - Vec3::from_array(source_origin);
                    let distance = offset.length();
                    if distance <= 1.0 || distance > FORCE_CANDIDATE_RADIUS {
                        continue;
                    }
                    // ForceThrow's broad target pass ultimately requires a
                    // forward dot >= 0.6 for clients. Level-1's exact trace is
                    // narrower, so this remains a candidate rather than proof.
                    if forward.dot(offset / distance) < 0.6 {
                        continue;
                    }
                    candidates.push((target.number, source_origin, toward_source));
                }
            }
            for (target, origin, toward_origin) in candidates {
                self.queue_impulse_candidate(
                    target,
                    ImpulseRagdollSource::Force,
                    origin,
                    toward_origin,
                    current_time,
                );
            }
        } else {
            // Still keep transition history current so re-enabling the option
            // doesn't synthesize an old Force gesture.
            for source in &all {
                self.force_gesture_anim.insert(
                    source.number,
                    source.state.field_i32("torsoAnim").unwrap_or(0),
                );
            }
        }

        let mut activations = Vec::new();
        for target in &all {
            let velocity = presented_entity_velocity(target);
            let previous = self.previous_impulse_velocity.get(&target.number).copied();
            let candidate = self.pending_impulse_ragdolls.get(&target.number).copied();
            let exact_local_knockback = followed_num == Some(target.number) && local_knockback;

            if !self.force_gripped_entities.contains(&target.number)
                && force_grip_target_alive(target)
            {
                if let (Some(candidate), Some(previous)) = (candidate, previous) {
                    let velocity_delta = Vec3::from_array(velocity) - Vec3::from_array(previous);
                    let delta_len = velocity_delta.length();
                    let target_from_origin =
                        Vec3::from_array(target.origin) - Vec3::from_array(candidate.origin);
                    let expected = if candidate.toward_origin {
                        -target_from_origin
                    } else {
                        target_from_origin
                    }
                    .normalize_or_zero();
                    let direction_matches = expected.length_squared() < 1.0e-6
                        || delta_len < 1.0e-6
                        || velocity_delta.normalize_or_zero().dot(expected) >= 0.20;
                    if exact_local_knockback
                        || (delta_len >= IMPULSE_DELTA_MIN && direction_matches)
                    {
                        activations.push((target.number, candidate.source, previous));
                    }
                }
            }
            self.previous_impulse_velocity
                .insert(target.number, velocity);
        }

        for (entity, source, seed_velocity) in activations {
            self.pending_impulse_ragdolls.remove(&entity);
            let expires_at = current_time.saturating_add(IMPULSE_RAGDOLL_MS);
            let was_active = self.active_impulse_ragdolls.contains_key(&entity);
            self.active_impulse_ragdolls
                .entry(entity)
                .and_modify(|active| {
                    active.expires_at = active.expires_at.max(expires_at);
                    active.source = source;
                })
                .or_insert(ActiveImpulseRagdoll {
                    source,
                    expires_at,
                    seed_velocity,
                });
            if self.ragdolls.debug_enabled() && !was_active {
                println!(
                    "RAPIER IMPULSE START: victim={} source={:?} seedVelocity=[{:.1},{:.1},{:.1}]",
                    entity, source, seed_velocity[0], seed_velocity[1], seed_velocity[2]
                );
            }
        }

        // OpenJK's BG_InKnockDownOnly is BOTH_KNOCKDOWN1..5. Once the
        // authoritative entity enters one of those animations, stop treating
        // the living player as a free impulse reaction and hand the exact
        // rendered Rapier pose into that already-advancing Ghoul2 animation.
        //
        // Require an existing Rapier instance so an impact and knockdown that
        // arrive in the same snapshot still get at least one genuine physics
        // presentation frame before the blend begins.
        let knockdown_handoffs = all
            .iter()
            .filter_map(|entity| {
                (self.active_impulse_ragdolls.contains_key(&entity.number)
                    && presented_entity_in_knockdown(entity)
                    && self.ragdolls.has_instance(entity.number))
                .then_some(entity.number)
            })
            .collect::<Vec<_>>();
        for entity in knockdown_handoffs {
            self.active_impulse_ragdolls.remove(&entity);
            self.pending_impulse_ragdolls.remove(&entity);
            self.ragdolls.begin_knockdown_recovery(entity, current_time);
            if self.ragdolls.debug_enabled() {
                println!("RAPIER IMPULSE -> KNOCKDOWN: victim={entity}");
            }
        }

        let expired = self
            .active_impulse_ragdolls
            .iter()
            .filter_map(|(&entity, active)| {
                (current_time >= active.expires_at || !live.contains(&entity)).then_some(entity)
            })
            .collect::<Vec<_>>();
        for entity in expired {
            self.active_impulse_ragdolls.remove(&entity);
            if live.contains(&entity) && !self.force_gripped_entities.contains(&entity) {
                self.ragdolls.begin_living_recovery(entity, current_time);
            }
            if self.ragdolls.debug_enabled() {
                println!("RAPIER IMPULSE END: victim={entity}");
            }
        }

        // Grip has stricter presentation semantics and always wins immediately.
        for entity in &self.force_gripped_entities {
            self.active_impulse_ragdolls.remove(entity);
            self.pending_impulse_ragdolls.remove(entity);
        }
    }

    /// Advance the visual physics world exactly once for this presentation frame.
    /// Ragdolls spawned later in the frame start from the exact current Ghoul2 pose.
    pub fn begin_physics_frame(&mut self, current_time: i32) {
        self.ragdolls.begin_frame(current_time);
    }

    pub fn perf_stats(&self) -> Ghoul2PerfStats {
        self.perf
    }

    pub fn set_skinning_mode(&mut self, mode: Ghoul2SkinningMode) {
        self.skinning_mode = mode;
    }

    pub fn skinning_mode(&self) -> Ghoul2SkinningMode {
        self.skinning_mode
    }

    pub fn set_early_frustum_cull(&mut self, enabled: bool) {
        self.early_frustum_cull = enabled;
    }

    /// Keep auxiliary presentation-only player runtimes alive when the ordinary
    /// snapshot player pass prunes stale centity state.
    pub fn set_external_live_entities(&mut self, entities: impl IntoIterator<Item = u16>) {
        self.external_live_entities.clear();
        self.external_live_entities.extend(entities);
    }

    pub fn set_rt_shadow_casters_enabled(&mut self, enabled: bool) {
        self.rt_shadow_casters_enabled = enabled;
    }

    /// Install the current map collision world for OpenJK-style blob traces.
    /// This is map-scoped and must not be rebound every frame.
    pub fn set_collision_world(&mut self, world: Option<CollisionWorld>) {
        self.collision_world = world;
        self.blob_shadow_instances.clear();
    }

    /// Begin one CG_AddEntities blob-shadow collection pass. All eligible
    /// players/NPCs are packed into one dynamic surface so the OpenJK visual
    /// costs one draw rather than one temporary-poly draw per character.
    pub fn begin_blob_shadow_frame(&mut self, enabled: bool) {
        self.blob_shadows_enabled = enabled;
        self.blob_shadow_instances.clear();
    }

    /// Finish cg_shadows 1. Keep the stock `markShadow` image *and its authored
    /// first-stage blend mode*, while batching all blobs into one surface.
    pub fn finish_blob_shadow_frame(&mut self) -> Option<DynamicModelSurface> {
        if !self.blob_shadows_enabled || self.blob_shadow_instances.is_empty() {
            return None;
        }
        if !self.blob_shadow_texture_resolved {
            let (texture, alpha_mode) = self.custom_shader_material("markShadow");
            self.blob_shadow_texture = texture;
            self.blob_shadow_alpha_mode = alpha_mode;
            self.blob_shadow_texture_resolved = true;
        }
        let Some(texture) = self.blob_shadow_texture.as_ref().map(Arc::clone) else {
            if !self.blob_shadow_asset_warned {
                eprintln!("BLOB SHADOW: OpenJK markShadow material/image was not found; suppressing blob draw");
                self.blob_shadow_asset_warned = true;
            }
            self.blob_shadow_instances.clear();
            return None;
        };
        Some(DynamicModelSurface {
            entity_num: u16::MAX,
            wireframe_class: DynamicWireframeClass::Effect,
            raster_visible: true,
            vertices: Arc::new(Vec::new()),
            indices: Arc::new(Vec::new()),
            lighting_origin: None,
            rt_rigid: None,
            rt_skinned_key: None,
            ghoul2_gpu: None,
            fx_gpu_sprites: Some(FxGpuSprites {
                instances: Arc::new(std::mem::take(&mut self.blob_shadow_instances)),
                blob_shadow: true,
            }),
            texture: Some(texture),
            // OpenJK registers `markShadow` as a shader and lets the renderer
            // honor the shader's authored blendFunc; preserve that here.
            alpha_mode: self.blob_shadow_alpha_mode,
        })
    }

    pub fn set_lod_scale(&mut self, lod_scale: f32) {
        self.lod_scale = if lod_scale.is_finite() {
            lod_scale.clamp(crate::fx::LOD_SCALE_MIN, crate::fx::LOD_SCALE_MAX)
        } else {
            crate::fx::LOD_SCALE_DEFAULT
        };
    }

    pub fn set_lod_bias(&mut self, lod_bias: i32) {
        // OpenJK's renderer cvar is not range-checked, but its Ghoul2 path
        // takes max(r_lodbias, modelBias). Our current per-model bias is 0,
        // so negative values have no effect; keep the stored runtime value in
        // the useful non-negative range.
        self.lod_bias = lod_bias.max(0);
    }

    /// `r_ghoul2animsmooth`. jaPRO stores this cvar unclamped and only enables
    /// smoothing when it reads strictly inside `(0, 1)` at use
    /// (`G2_TransformGhoulBones`'s `val>0.0f&&val<1.0f` gate); `1.0` or higher
    /// is therefore a no-op in jaPRO, not "maximum smoothing". Mirror that by
    /// storing the raw value here and gating activation at the call site,
    /// rather than clamping into `[0, 1)` and making `1.0` mean "near-frozen".
    pub fn set_ghoul2_anim_smooth(&mut self, factor: f32) {
        self.ghoul2_anim_smooth = if factor.is_finite() {
            factor.max(0.0)
        } else {
            0.0
        };
    }
}
