//! Events.
use crate::cgame::player_presenter::{
    body_queue_model1_weapon, BodyQueueCopyState, EntityEvent, Ghoul2ServerCommand,
    PlayerPresenter, PresentationEvent, ET_BODY, MAX_CLIENTS, WP_SABER,
};

impl PlayerPresenter {
    /// OpenJK CG_RestoreClientGhoul_f / CG_BodyQueueCopy presentation state.
    pub fn apply_ghoul2_server_command(&mut self, command: Ghoul2ServerCommand) {
        match command {
            Ghoul2ServerCommand::BodyQueueCopy {
                source_client,
                body_entity,
                known_weapon,
                light_side,
            } => {
                // CG_RestoreClientGhoul_f returns immediately when the source
                // has no Ghoul2 instance. `entities` is this presenter's
                // equivalent persistent instance state, so do the same here.
                let Some(source) = self.entities.get(&source_client) else {
                    return;
                };
                let source_model1 = if source.primary_saber_attached {
                    Some(WP_SABER)
                } else {
                    source.attached_weapon
                };
                let source_model2_saber = source.secondary_saber_attached;

                // OpenJK/TaystJK CG_BodyQueueCopy calls
                // G2API_DuplicateGhoul2Instance(source->ghoul2, &body->ghoul2)
                // before it mutates either instance.  Our EntityPlayerState is
                // the persistent Ghoul2-equivalent owner of the animator/model
                // state, so duplicate it into the ET_BODY slot here as well.
                //
                // Without this, the first ET_BODY presentation constructs a
                // brand-new PlayerAnimationState at the respawn time.  Its
                // death animation therefore starts again from frame zero even
                // though the player already died once.  The Rapier articulation
                // transfer below hides that only when a live corpse instance is
                // available; this copy also preserves the correct pose if that
                // instance was culled/retired/not spawned.
                let mut body_runtime = source.clone();
                // Bone smoothing is renderer cache state, not part of the
                // duplicated Ghoul2 instance.  Do not blend a body-queue slot
                // against the source entity's previous raster history.
                body_runtime.bone_smooth_history = None;

                // CG_BodyQueueCopy duplicates the actual source Ghoul2 first,
                // then applies knownWeapon only to model index 1.
                // A reused body-queue slot starts a fresh life.
                self.entities.insert(body_entity, body_runtime);
                // Duplicate the persistent UV injuries with the corpse, then
                // reset the live player's damage for its next spawn.
                self.skin_gore.body_queue_copy(source_client, body_entity);
                self.gore.retain(|mark| mark.entity != body_entity);
                for mark in &mut self.gore {
                    if mark.entity == source_client {
                        mark.entity = body_entity;
                    }
                }
                self.pending_gore.retain(|hit| hit.entity != body_entity && hit.entity != source_client);
                self.saber_gore_hits.retain(|attacker, hit| {
                    ![source_client, body_entity].contains(attacker)
                        && ![source_client, body_entity].contains(&hit.target)
                });
                self.body_fade.remove(&body_entity);
                self.disintegration_start.remove(&body_entity);
                self.disintegration_smoke_last.remove(&body_entity);
                if let Some(parts) = self.dismembered.get(&source_client).cloned() {
                    self.dismembered.insert(body_entity, parts);
                } else {
                    self.dismembered.remove(&body_entity);
                }
                // Preserve the already-solved corpse articulation across the
                // live-player -> ET_BODY ownership transfer. OpenJK copies the
                // existing Ghoul2 instance here rather than replaying death.
                self.ragdolls.body_queue_copy(source_client, body_entity);
                self.body_queue_copies.insert(
                    body_entity,
                    BodyQueueCopyState {
                        source_client,
                        _known_weapon: known_weapon,
                        _light_side: light_side,
                        model1_weapon: body_queue_model1_weapon(source_model1, known_weapon),
                        model2_saber: source_model2_saber,
                    },
                );

                // The command handler resets the live centity tracking fields
                // after the body has been copied. It does not remove the source
                // model slots themselves.
                if let Some(source) = self.entities.get_mut(&source_client) {
                    source.cent_weapon = 0;
                    source.ghoul2_weapon = None;
                }
                self.thrown_sabers.remove(&source_client);
            }
            Ghoul2ServerCommand::RestoreClient { source_client } => {
                // Same early return as CG_RestoreClientGhoul_f.
                let Some(source) = self.entities.get_mut(&source_client) else {
                    return;
                };
                source.cent_weapon = 0;
                source.ghoul2_weapon = None;
                self.thrown_sabers.remove(&source_client);
                self.skin_gore.clear_entity(source_client);
                self.gore.retain(|mark| mark.entity != source_client);
                self.pending_gore.retain(|hit| hit.entity != source_client);
                self.saber_gore_hits.retain(|attacker, hit| *attacker != source_client && hit.target != source_client);
                // CG_ReattachLimb restores the live player's original Ghoul2
                // surface flags after the body queue copy. The corpse retains
                // the cloned cut state above.
                self.dismembered.remove(&source_client);
            }
            Ghoul2ServerCommand::KillEntity { entity } => {
                // CG_KillGhoul2_f / CG_KillCEntityG2: drop the persistent
                // presentation instance. If a later snapshot still references
                // this slot it will be recreated from its current entity state.
                self.entities.remove(&entity);
                self.ragdolls.remove_entity(entity);
                self.body_queue_copies.remove(&entity);
                self.body_fade.remove(&entity);
                self.disintegration_start.remove(&entity);
                self.disintegration_smoke_last.remove(&entity);
                self.dismembered.remove(&entity);
                self.dismember_source_snaps.remove(&entity);
                self.dynamic_gib_sources.remove(&entity);
                self.detached_limb_visuals.remove(&entity);
                self.thrown_sabers.remove(&entity);
                self.team_power.remove(&entity);
                self.force_grip_targets.remove(&entity);
                self.force_grip_targets
                    .retain(|_, target| *target != entity);
                self.force_gripped_entities.remove(&entity);
                self.pending_impulse_ragdolls.remove(&entity);
                self.active_impulse_ragdolls.remove(&entity);
                self.previous_impulse_velocity.remove(&entity);
                self.force_gesture_anim.remove(&entity);
                self.last_gpu.remove(&entity);
                self.skin_gore.clear_entity(entity);
                self.gore.retain(|mark| mark.entity != entity);
                self.pending_gore.retain(|hit| hit.entity != entity);
                self.saber_gore_hits.retain(|attacker, hit| *attacker != entity && hit.target != entity);
                self.vehicle_snaps.remove(&entity);
                // CG_KillGhoul2_f invalidates the Ghoul2 instance and its
                // animation channels. Do not let a recycled entity slot reuse
                // its old vehicle lerp frames (even for the same model).
                self.vehicle_animations.remove(&entity);
                self.player_diagnostics.remove(&entity);
            }
        }
    }

    /// Apply server events that affect Ghoul2 presentation and its marks.
    pub fn apply_entity_event(&mut self, event: &PresentationEvent, current_time: i32) {
        self.observe_impulse_event(event);
        let state = &event.state;
        let until = event.server_time.saturating_add(1000);
        match event.event {
            EntityEvent::EV_JK2_BODY_QUEUE_COPY => {
                // MVSDK CG_EntityEvent: eventParm is the source client;
                // the event rides on the destination ET_BODY. The adapter
                // already translated knownWeapon into presentation IDs.
                if self.assets.jk2_version().is_none()
                    || state.field_i32("eType") != Some(ET_BODY as i32)
                {
                    return;
                }
                let Ok(source_client) = u16::try_from(event.parm) else { return; };
                let body_entity = event.entity_num;
                if usize::from(source_client) >= MAX_CLIENTS
                    || usize::from(body_entity) < MAX_CLIENTS
                {
                    return;
                }
                // MVSDK cleans the destination Ghoul2 even when the source
                // is unavailable (e.g. first snapshot or entering PVS).
                self.apply_ghoul2_server_command(Ghoul2ServerCommand::KillEntity {
                    entity: body_entity,
                });
                self.apply_ghoul2_server_command(Ghoul2ServerCommand::BodyQueueCopy {
                    source_client,
                    body_entity,
                    known_weapon: state.field_i32("weapon").unwrap_or(0),
                    light_side: false, // JK2 has no JKA body-fade side parameter.
                });
                // CG_ReattachLimb: only the copied corpse retains severed limbs.
                self.dismembered.remove(&source_client);
                return;
            }
            EntityEvent::EV_FORCE_DRAINED => {
                if let Ok(owner) = u16::try_from(state.field_i32("owner").unwrap_or(-1)) {
                    self.team_power.insert(owner, (until, 2));
                }
                return;
            }
            EntityEvent::EV_TEAM_POWER => {
                // CG_InClientBitflags: clients 0-15 in trickedentindex, 16-31 in trickedentindex2.
                let low = state.field_i32("trickedentindex").unwrap_or(0);
                let high = state.field_i32("trickedentindex2").unwrap_or(0);
                for client in 0u16..32 {
                    let bits = if client > 15 {
                        high >> (client - 16)
                    } else {
                        low >> client
                    };
                    if bits & 1 != 0 {
                        // eventParm 1 is heal, anything else force regen.
                        self.team_power
                            .insert(client, (until, u8::from(event.parm == 1)));
                    }
                }
                return;
            }
            EntityEvent::EV_VEH_FIRE => {
                self.vehicle_muzzle_fire(event);
                return;
            }
            EntityEvent::EV_GHOUL2_MARK => {
                self.add_gore_mark(event);
                return;
            }
            EntityEvent::EV_SABER_HIT => {
                self.note_saber_gore_hit(event, current_time);
                return;
            }
            EntityEvent::EV_PAIN
                if self.skin_gore_enabled
                    && matches!(
                        state.field_i32("eType"),
                        Some(crate::cgame::ET_PLAYER | crate::cgame::ET_NPC)
                    ) =>
            {
                let victim = event.entity_num;
                let time = event.server_time;
                if !self.skin_gore_pains.iter().any(|&(number, stamp, _)| {
                    number == victim && stamp == time
                }) {
                    self.skin_gore_pains.push((victim, time, event.parm));
                }
                return;
            }
            // TaystJK/OpenJK CG_EntityEvent also invokes CG_G2MarkEvent for
            // flagged projectile hits and misses (cg_event.c). These are NOT
            // necessarily accompanied by an EV_GHOUL2_MARK event. Without
            // this path normal blaster hits on players/NPCs never leave marks.
            EntityEvent::EV_MISSILE_HIT | EntityEvent::EV_MISSILE_MISS
                if state.field_i32("trickedentindex").unwrap_or(0) != 0 =>
            {
                self.add_gore_mark(event);
                return;
            }
            EntityEvent::EV_BODYFADE => {
                if state.field_i32("eType").unwrap_or(0) == ET_BODY as i32 {
                    self.body_fade.insert(event.entity_num, event.server_time);
                }
                return;
            }
            EntityEvent::EV_PREDEFSOUND if event.parm == 3 => {
                // PDSOUND_ABSORBHIT: the absorbing client flashes its shield.
                if let Ok(client) = u16::try_from(state.field_i32("trickedentindex").unwrap_or(-1))
                {
                    if client < 32 {
                        self.team_power.insert(client, (until, 3));
                    }
                }
                return;
            }
            _ => {}
        }
        if event.event != EntityEvent::EV_DESTROY_WEAPON_MODEL {
            return;
        }
        let Ok(target) = u16::try_from(event.parm) else {
            return;
        };
        if let Some(runtime) = self.entities.get_mut(&target) {
            runtime.attached_weapon = None;
            runtime.primary_saber_attached = false;
        }
    }
}
