//! Owner-driven orientation from OpenJK CG_Player's thrown-saber branch.
//! Keep its local pitch/initialization state out of authoritative snapshots.

use super::player_presenter::openjk_vectoangles;
use super::{evaluate_trajectory, trajectory, PresentedEntity};

#[derive(Debug)]
pub(super) struct SaberThrowState {
    saber_entity: u16,
    model_index: i32,
    client_num: i32,
    teleport_bit: i32,
    initial_snapshot: i32,
    initial_time: i32,
    last_time: i32,
    pitch: f32,
}

impl SaberThrowState {
    pub(super) fn new(
        owner: &PresentedEntity,
        saber: &PresentedEntity,
        time: i32,
        snapshot: i32,
    ) -> Self {
        let pitch = trajectory(&saber.state, "apos").base[0];
        Self {
            saber_entity: saber.number,
            model_index: saber.state.field_i32("modelindex").unwrap_or(0),
            client_num: owner.state.field_i32("clientNum").unwrap_or(-1),
            teleport_bit: owner.state.field_i32("eFlags").unwrap_or(0) & (1 << 3),
            initial_snapshot: snapshot,
            initial_time: time,
            last_time: time,
            // CG_Player uses zero as the uninitialized bolt3 sentinel.
            pitch: if pitch == 0.0 { 1.0 } else { pitch },
        }
    }

    pub(super) fn angles(
        &mut self,
        owner: &PresentedEntity,
        saber: &PresentedEntity,
        time: i32,
        snapshot: i32,
        return_damage: bool,
    ) -> Result<[f32; 3], String> {
        if self.saber_entity != saber.number
            || self.model_index != saber.state.field_i32("modelindex").unwrap_or(0)
            || self.client_num != owner.state.field_i32("clientNum").unwrap_or(-1)
            || self.teleport_bit != (owner.state.field_i32("eFlags").unwrap_or(0) & (1 << 3))
            || time < self.last_time
        {
            *self = Self::new(owner, saber, time, snapshot);
        }
        // CG_Player bolt3 approaches 90 at 0.5 degrees per millisecond.
        let step = time.saturating_sub(self.last_time) as f32 * 0.5;
        self.pitch += (90.0 - self.pitch).clamp(-step, step);
        self.last_time = time;

        let mut angular = trajectory(&saber.state, "apos");
        angular.base[0] = self.pitch;
        // Registration rewrites apos.trTime and marks bolt2=123 until the
        // next snapshot replaces currentState. Do not restart yaw each frame.
        let initializing = snapshot == self.initial_snapshot;
        if initializing {
            angular.time = self.initial_time;
        }
        if !initializing
            && saber.state.field_i32("saberInFlight").unwrap_or(0) == 0
            && saber.state.field_i32("bolt2").unwrap_or(0) != 123
            && (!return_damage || owner.state.field_i32("saberHolstered").unwrap_or(0) != 0)
        {
            let mut direction = openjk_vectoangles([
                saber.origin[0] - owner.origin[0],
                saber.origin[1] - owner.origin[1],
                saber.origin[2] - owner.origin[2],
            ]);
            direction[0] += 90.0;
            angular.base = direction;
            angular.delta = [0.0; 3];
        }
        // CG_ManualEntityRender evaluates the overridden angular trajectory.
        // The server supplies the spin rate, not this presentation code.
        evaluate_trajectory(angular, time)
    }
}

/// CG_Player zeros roll for blade bolt queries after rendering the hilt.
pub(super) fn blade_angles(mut hilt_angles: [f32; 3]) -> [f32; 3] {
    hilt_angles[2] = 0.0;
    hilt_angles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cgame::{set_entity_i32, set_entity_vec3, ET_GENERAL, ET_PLAYER, TR_LINEAR};
    use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};

    fn entities() -> (PresentedEntity, PresentedEntity) {
        let mut owner = PresentedEntity {
            number: 2,
            entity_type: ET_PLAYER,
            origin: [0.0; 3],
            angles: [0.0; 3],
            state: EntityState {
                number: 2,
                fields: [0; ENTITY_FIELDS.len()],
            },
        };
        set_entity_i32(&mut owner.state, "clientNum", 2);
        let mut saber = PresentedEntity {
            number: 48,
            entity_type: ET_GENERAL,
            origin: [100.0, 0.0, 0.0],
            angles: [0.0; 3],
            state: EntityState {
                number: 48,
                fields: [0; ENTITY_FIELDS.len()],
            },
        };
        set_entity_i32(&mut saber.state, "apos.trType", TR_LINEAR);
        set_entity_i32(&mut saber.state, "apos.trTime", 900);
        set_entity_vec3(&mut saber.state, "apos.trBase", [0.0, 20.0, 7.0]);
        set_entity_vec3(&mut saber.state, "apos.trDelta", [0.0, 720.0, 0.0]);
        set_entity_i32(&mut saber.state, "saberInFlight", 1);
        (owner, saber)
    }

    #[test]
    fn pitch_settles_and_spin_uses_server_velocity_and_time() {
        let (owner, saber) = entities();
        let mut state = SaberThrowState::new(&owner, &saber, 1000, 10);
        assert_eq!(
            state.angles(&owner, &saber, 1000, 10, false).unwrap(),
            [1.0, 20.0, 7.0]
        );
        let angles = state.angles(&owner, &saber, 1100, 10, false).unwrap();
        assert_eq!(angles, [51.0, 92.0, 7.0]);
        // Next snapshot restores the server's angular epoch (900, not 1000).
        let angles = state.angles(&owner, &saber, 1200, 11, false).unwrap();
        assert!((angles[1] - 236.0).abs() < 0.001);
        assert_eq!(angles[0], 90.0);
        assert_eq!(blade_angles(angles)[2], 0.0);
        assert_eq!(
            state.angles(&owner, &saber, 1200, 11, false).unwrap(),
            angles
        );
    }

    #[test]
    fn return_faces_owner_except_return_damage_and_holster_overrides_it() {
        let (mut owner, mut saber) = entities();
        let mut state = SaberThrowState::new(&owner, &saber, 1000, 10);
        set_entity_i32(&mut saber.state, "saberInFlight", 0);
        assert_eq!(
            state.angles(&owner, &saber, 1200, 11, false).unwrap(),
            [90.0, 0.0, 0.0]
        );
        let spinning = state.angles(&owner, &saber, 1200, 11, true).unwrap();
        assert!((spinning[1] - 236.0).abs() < 0.001);
        set_entity_i32(&mut owner.state, "saberHolstered", 1);
        assert_eq!(
            state.angles(&owner, &saber, 1200, 11, true).unwrap(),
            [90.0, 0.0, 0.0]
        );
        saber.origin = [0.0, 100.0, 100.0];
        assert_eq!(
            state.angles(&owner, &saber, 1210, 12, false).unwrap(),
            [45.0, 90.0, 0.0]
        );
        set_entity_i32(&mut saber.state, "bolt2", 123);
        assert_ne!(
            state.angles(&owner, &saber, 1210, 12, false).unwrap()[1],
            90.0
        );
    }

    #[test]
    fn orientation_resets_on_rewind_model_change_and_entity_reuse() {
        let (mut owner, mut saber) = entities();
        let mut state = SaberThrowState::new(&owner, &saber, 1000, 10);
        assert_eq!(
            state.angles(&owner, &saber, 1300, 11, false).unwrap()[0],
            90.0
        );
        assert_eq!(
            state.angles(&owner, &saber, 1000, 10, false).unwrap()[0],
            1.0
        );
        state.angles(&owner, &saber, 1300, 11, false).unwrap();
        set_entity_i32(&mut saber.state, "modelindex", 5);
        assert_eq!(
            state.angles(&owner, &saber, 1400, 12, false).unwrap()[0],
            1.0
        );
        state.angles(&owner, &saber, 1700, 13, false).unwrap();
        set_entity_i32(&mut owner.state, "clientNum", 4);
        assert_eq!(
            state.angles(&owner, &saber, 1800, 14, false).unwrap()[0],
            1.0
        );
        state.angles(&owner, &saber, 2100, 15, false).unwrap();
        saber.number = 50;
        assert_eq!(
            state.angles(&owner, &saber, 2200, 16, false).unwrap()[0],
            1.0
        );
        state.angles(&owner, &saber, 2500, 17, false).unwrap();
        set_entity_i32(&mut owner.state, "eFlags", 1 << 3);
        assert_eq!(
            state.angles(&owner, &saber, 2600, 18, false).unwrap()[0],
            1.0
        );
    }

    #[test]
    fn pitch_is_frame_rate_independent_and_zero_velocity_does_not_invent_spin() {
        let (owner, mut saber) = entities();
        set_entity_vec3(&mut saber.state, "apos.trBase", [160.0, 20.0, 0.0]);
        set_entity_vec3(&mut saber.state, "apos.trDelta", [0.0; 3]);
        let mut one_step = SaberThrowState::new(&owner, &saber, 1000, 10);
        let mut many_steps = SaberThrowState::new(&owner, &saber, 1000, 10);
        for time in (1010..=1100).step_by(10) {
            many_steps.angles(&owner, &saber, time, 11, false).unwrap();
        }
        let expected = one_step.angles(&owner, &saber, 1100, 11, false).unwrap();
        assert_eq!(expected, [110.0, 20.0, 0.0]);
        assert_eq!(
            many_steps.angles(&owner, &saber, 1100, 11, false).unwrap(),
            expected
        );
        assert_eq!(
            many_steps.angles(&owner, &saber, 1500, 12, false).unwrap(),
            [90.0, 20.0, 0.0]
        );
    }
}
