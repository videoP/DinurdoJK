//! Events.
use crate::cgame::player_presenter::{
    body_queue_model1_weapon, BodyQueueCopyState, EntityEvent, Ghoul2ServerCommand,
    PlayerPresenter, PresentationEvent, ET_BODY, WP_SABER,
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

                // CG_BodyQueueCopy duplicates the actual source Ghoul2 first,
                // then applies knownWeapon only to model index 1.
                // A reused body-queue slot starts a fresh life.
                self.body_fade.remove(&body_entity);
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
                self.dismembered.remove(&entity);
                self.dismember_source_snaps.remove(&entity);
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
                self.vehicle_snaps.remove(&entity);
                self.player_diagnostics.remove(&entity);
            }
        }
    }

    /// OpenJK EV_DESTROY_WEAPON_MODEL removes Ghoul2 model index 1 only.
    pub fn apply_entity_event(&mut self, event: &PresentationEvent) {
        self.observe_impulse_event(event);
        let state = &event.state;
        let until = event.server_time.saturating_add(1000);
        match event.event {
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
