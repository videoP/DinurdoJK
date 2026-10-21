//! Keep the accepted native taunt's cancellation rules after its animation expires.

#[derive(Clone, Copy, Debug)]
pub(super) struct Taunt {
    torso: i32,
    legs: i32,
}

impl Taunt {
    pub(super) fn from_animations(torso: i32, legs: i32) -> Option<Self> {
        (crate::emote_net::is_normal_taunt(torso) || crate::emote_net::is_normal_taunt(legs))
            .then_some(Self {
                torso: torso & 0x7fff,
                legs: legs & 0x7fff,
            })
    }

    pub(super) fn cancel_input(self, buttons: i32, up_move: i8) -> bool {
        jka_movement::taunt_should_cancel(self.torso, self.legs, buttons, up_move)
    }

    pub(super) fn interrupted(&mut self, torso: i32, legs: i32) -> bool {
        // PM_Weapon can begin on the torso while velocity settles, then take
        // over both channels once stationary. Retain that ownership afterwards;
        // a later leg change may be the cancellation we need to detect.
        if torso & 0x7fff == self.torso && jka_movement::is_full_body_taunt(torso, legs) {
            self.legs = legs & 0x7fff;
        }
        // A torso-only taunt allows independent leg animations, including crouching.
        // Full-body taunts own both channels. Idle/locomotion after natural expiry
        // must not terminate a longer cosmetic performance.
        !Self::channel_continues(self.torso, torso)
            || (jka_movement::is_full_body_taunt(self.torso, self.legs)
                && !Self::channel_continues(self.legs, legs))
    }

    fn channel_continues(source: i32, anim: i32) -> bool {
        let anim = anim & 0x7fff;
        if anim == source {
            return true;
        }
        let Some(name) = jka_assets::animation::AnimationSet::name(anim) else {
            return false;
        };
        [
            "BOTH_STAND",
            "BOTH_SABERFAST_STANCE",
            "BOTH_SABERSLOW_STANCE",
            "BOTH_SABERDUAL_STANCE",
            "BOTH_SABERSTAFF_STANCE",
            "BOTH_RUN",
            "BOTH_WALK",
            "BOTH_TURN",
            "TORSO_WEAPONIDLE",
            "TORSO_WEAPONREADY",
            // PM_Weapon raises a gun after HANDEXTEND_WEAPONREADY on natural expiry.
            "TORSO_RAISEWEAP",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    }
}
