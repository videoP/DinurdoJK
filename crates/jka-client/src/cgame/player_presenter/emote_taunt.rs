//! A replaced taunt must not reappear when a shorter cosmetic clip finishes.
use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};

#[derive(Clone, Default)]
pub(super) struct EmoteTauntMask {
    // Raw animation ID (including toggle bit) and restart flip, per channel.
    replaced: [Option<(i32, i32)>; 2],
}

impl EmoteTauntMask {
    pub(super) fn animation_state(
        &mut self,
        entity: &EntityState,
        emote_playing: bool,
    ) -> Option<EntityState> {
        if !emote_playing && self.replaced == [None; 2] {
            return None;
        }
        let mut masked = None;
        let channels = [("torsoAnim", "torsoFlip"), ("legsAnim", "legsFlip")];
        for (channel, (anim_field, flip_field)) in channels.into_iter().enumerate() {
            let anim = entity.field_i32(anim_field).unwrap_or(-1);
            let stamp = (anim, entity.field_i32(flip_field).unwrap_or(0));
            if self.replaced[channel] != Some(stamp) {
                self.replaced[channel] = None;
            }
            if emote_playing && crate::emote_net::is_normal_taunt(anim) {
                self.replaced[channel] = Some(stamp);
            }
            // A taunt can acquire the legs after residual velocity settles,
            // even after a short cosmetic clip has already finished. Follow
            // the still-matching channel into that same full-body taunt.
            let (other_anim, other_flip) = channels[1 - channel];
            let other = (
                entity.field_i32(other_anim).unwrap_or(-1),
                entity.field_i32(other_flip).unwrap_or(0),
            );
            if channel == 1
                && self.replaced[1 - channel] == Some(other)
                && anim & 0x7fff == other.0 & 0x7fff
                && jka_movement::is_full_body_taunt(anim, other.0)
            {
                self.replaced[channel] = Some(stamp);
            }
            if !emote_playing && self.replaced[channel].is_some() {
                let idle = idle_animation(entity, anim);
                let state = masked.get_or_insert_with(|| entity.clone());
                let field = ENTITY_FIELDS
                    .iter()
                    .position(|(name, _)| *name == anim_field)
                    .unwrap();
                state.fields[field] = idle as u32;
            }
        }
        masked
    }
}

fn idle_animation(entity: &EntityState, taunt: i32) -> i32 {
    let name =
        if entity.field_i32("weapon") == Some(3) && entity.field_i32("saberHolstered") != Some(2) {
            match jka_assets::animation::AnimationSet::name(taunt & 0x7fff) {
                Some("BOTH_DUAL_TAUNT") => "BOTH_SABERDUAL_STANCE",
                Some("BOTH_STAFF_TAUNT") => "BOTH_SABERSTAFF_STANCE",
                _ => "BOTH_STAND2",
            }
        } else {
            "BOTH_STAND1"
        };
    jka_assets::animation::animation_index(name).unwrap() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(entity: &mut EntityState, name: &str, value: i32) {
        let field = ENTITY_FIELDS
            .iter()
            .position(|(key, _)| *key == name)
            .unwrap();
        entity.fields[field] = value as u32;
    }

    fn anim(name: &str) -> i32 {
        jka_assets::animation::animation_index(name).unwrap() as i32
    }

    fn dual() -> EntityState {
        let mut entity = EntityState {
            number: 1,
            fields: [0; ENTITY_FIELDS.len()],
        };
        for (key, value) in [
            ("torsoAnim", anim("BOTH_DUAL_TAUNT")),
            ("legsAnim", anim("BOTH_DUAL_TAUNT")),
            ("weapon", 3),
        ] {
            set(&mut entity, key, value);
        }
        entity
    }

    #[test]
    fn short_emote_returns_to_idle_until_replaced_taunt_ends() {
        let mut mask = EmoteTauntMask::default();
        let mut entity = dual();
        let original = entity.clone();
        assert!(mask.animation_state(&entity, true).is_none());
        for _ in 0..3 {
            let visual = mask.animation_state(&entity, false).unwrap();
            assert_eq!(
                visual.field_i32("torsoAnim"),
                Some(anim("BOTH_SABERDUAL_STANCE"))
            );
            assert_eq!(
                visual.field_i32("legsAnim"),
                Some(anim("BOTH_SABERDUAL_STANCE"))
            );
            assert_eq!(entity, original); // Presentation never changes native state.
        }
        set(&mut entity, "legsAnim", anim("BOTH_STAND1"));
        set(&mut entity, "torsoAnim", anim("BOTH_STAND1"));
        assert!(mask.animation_state(&entity, false).is_none());
        entity = original;
        assert!(mask.animation_state(&entity, false).is_none()); // A later ordinary taunt is visible.
    }

    #[test]
    fn torso_only_remap_preserves_crouch_and_new_taunt_flips_are_visible() {
        let mut mask = EmoteTauntMask::default();
        let mut entity = dual();
        set(&mut entity, "torsoAnim", anim("BOTH_ENGAGETAUNT"));
        set(&mut entity, "legsAnim", anim("BOTH_CROUCH1"));
        mask.animation_state(&entity, true);
        let visual = mask.animation_state(&entity, false).unwrap();
        assert_eq!(visual.field_i32("torsoAnim"), Some(anim("BOTH_STAND2")));
        assert_eq!(visual.field_i32("legsAnim"), Some(anim("BOTH_CROUCH1")));
        set(&mut entity, "torsoFlip", 1);
        assert!(mask.animation_state(&entity, false).is_none());
    }

    #[test]
    fn full_body_takeover_after_clip_completion_stays_hidden() {
        let mut mask = EmoteTauntMask::default();
        let mut entity = dual();
        set(&mut entity, "legsAnim", anim("BOTH_STAND1"));
        mask.animation_state(&entity, true);
        let visual = mask.animation_state(&entity, false).unwrap();
        assert_eq!(visual.field_i32("legsAnim"), Some(anim("BOTH_STAND1")));
        set(&mut entity, "legsAnim", anim("BOTH_DUAL_TAUNT"));
        set(&mut entity, "legsFlip", 1);
        let visual = mask.animation_state(&entity, false).unwrap();
        assert_eq!(
            visual.field_i32("torsoAnim"),
            Some(anim("BOTH_SABERDUAL_STANCE"))
        );
        assert_eq!(
            visual.field_i32("legsAnim"),
            Some(anim("BOTH_SABERDUAL_STANCE"))
        );
    }

    #[test]
    fn long_emote_does_not_hide_a_taunt_started_after_native_expiry() {
        let mut mask = EmoteTauntMask::default();
        let mut entity = dual();
        mask.animation_state(&entity, true);
        set(&mut entity, "torsoAnim", anim("BOTH_STAND1"));
        set(&mut entity, "legsAnim", anim("BOTH_STAND1"));
        assert!(mask.animation_state(&entity, true).is_none());
        assert!(mask.animation_state(&entity, false).is_none());
        assert!(mask.animation_state(&dual(), false).is_none());
    }
}
