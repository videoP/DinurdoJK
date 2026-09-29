//! Jedi Academy multiplayer `entity_event_t` ported from OpenJK
//! `codemp/game/bg_public.h`.
//!
//! These discriminants are protocol-visible. Keep the ordering exactly aligned
//! with OpenJK protocol 26. The two high event bits are uniqueness toggles and
//! are stripped before converting a wire value to [`EntityEvent`].

pub const EV_EVENT_BIT1: i32 = 0x0000_0100;
pub const EV_EVENT_BIT2: i32 = 0x0000_0200;
pub const EV_EVENT_BITS: i32 = EV_EVENT_BIT1 | EV_EVENT_BIT2;
pub const EVENT_VALID_MSEC: i32 = 300;

macro_rules! entity_events {
    ($($name:ident),+ $(,)?) => {
        #[repr(i32)]
        #[allow(non_camel_case_types)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum EntityEvent {
            $($name,)+
            EV_NUM_ENTITY_EVENTS,
        }

        impl EntityEvent {
            /// OpenJK `EV_NUM_ENTITY_EVENTS`; the sentinel itself is not a
            /// transmittable semantic event.
            pub const COUNT: usize = Self::EV_NUM_ENTITY_EVENTS as usize;

            /// Every real OpenJK entity event in protocol order.
            pub const ALL: [Self; Self::COUNT] = [
                $(Self::$name,)+
            ];

            #[inline]
            pub const fn as_i32(self) -> i32 {
                self as i32
            }

            #[inline]
            pub fn from_i32(value: i32) -> Option<Self> {
                usize::try_from(value)
                    .ok()
                    .and_then(|index| Self::ALL.get(index).copied())
            }

            #[inline]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$name => stringify!($name),)+
                    Self::EV_NUM_ENTITY_EVENTS => "EV_NUM_ENTITY_EVENTS",
                }
            }
        }

        impl TryFrom<i32> for EntityEvent {
            type Error = i32;

            #[inline]
            fn try_from(value: i32) -> Result<Self, Self::Error> {
                Self::from_i32(value).ok_or(value)
            }
        }

        impl From<EntityEvent> for i32 {
            #[inline]
            fn from(value: EntityEvent) -> Self {
                value.as_i32()
            }
        }
    };
}

entity_events!(
    EV_NONE,
    EV_CLIENTJOIN,
    EV_FOOTSTEP,
    EV_FOOTSTEP_METAL,
    EV_FOOTSPLASH,
    EV_FOOTWADE,
    EV_SWIM,
    EV_STEP_4,
    EV_STEP_8,
    EV_STEP_12,
    EV_STEP_16,
    EV_FALL,
    EV_JUMP_PAD,
    EV_GHOUL2_MARK,
    EV_GLOBAL_DUEL,
    EV_PRIVATE_DUEL,
    EV_JUMP,
    EV_ROLL,
    EV_WATER_TOUCH,
    EV_WATER_LEAVE,
    EV_WATER_UNDER,
    EV_WATER_CLEAR,
    EV_ITEM_PICKUP,
    EV_GLOBAL_ITEM_PICKUP,
    EV_VEH_FIRE,
    EV_NOAMMO,
    EV_CHANGE_WEAPON,
    EV_FIRE_WEAPON,
    EV_ALT_FIRE,
    EV_SABER_ATTACK,
    EV_SABER_HIT,
    EV_SABER_BLOCK,
    EV_SABER_CLASHFLARE,
    EV_SABER_UNHOLSTER,
    EV_BECOME_JEDIMASTER,
    EV_DISRUPTOR_MAIN_SHOT,
    EV_DISRUPTOR_SNIPER_SHOT,
    EV_DISRUPTOR_SNIPER_MISS,
    EV_DISRUPTOR_HIT,
    EV_DISRUPTOR_ZOOMSOUND,
    EV_PREDEFSOUND,
    EV_TEAM_POWER,
    EV_SCREENSHAKE,
    EV_LOCALTIMER,
    EV_USE,
    EV_USE_ITEM0,
    EV_USE_ITEM1,
    EV_USE_ITEM2,
    EV_USE_ITEM3,
    EV_USE_ITEM4,
    EV_USE_ITEM5,
    EV_USE_ITEM6,
    EV_USE_ITEM7,
    EV_USE_ITEM8,
    EV_USE_ITEM9,
    EV_USE_ITEM10,
    EV_USE_ITEM11,
    EV_USE_ITEM12,
    EV_USE_ITEM13,
    EV_USE_ITEM14,
    EV_USE_ITEM15,
    EV_ITEMUSEFAIL,
    EV_ITEM_RESPAWN,
    EV_ITEM_POP,
    EV_PLAYER_TELEPORT_IN,
    EV_PLAYER_TELEPORT_OUT,
    EV_GRENADE_BOUNCE,
    EV_MISSILE_STICK,
    EV_PLAY_EFFECT,
    EV_PLAY_EFFECT_ID,
    EV_PLAY_PORTAL_EFFECT_ID,
    EV_PLAYDOORSOUND,
    EV_PLAYDOORLOOPSOUND,
    EV_BMODEL_SOUND,
    EV_MUTE_SOUND,
    EV_VOICECMD_SOUND,
    EV_GENERAL_SOUND,
    EV_GLOBAL_SOUND,
    EV_GLOBAL_TEAM_SOUND,
    EV_ENTITY_SOUND,
    EV_PLAY_ROFF,
    EV_GLASS_SHATTER,
    EV_DEBRIS,
    EV_MISC_MODEL_EXP,
    EV_CONC_ALT_IMPACT,
    EV_MISSILE_HIT,
    EV_MISSILE_MISS,
    EV_MISSILE_MISS_METAL,
    EV_BULLET,
    EV_PAIN,
    EV_DEATH1,
    EV_DEATH2,
    EV_DEATH3,
    EV_OBITUARY,
    EV_POWERUP_QUAD,
    EV_POWERUP_BATTLESUIT,
    EV_FORCE_DRAINED,
    EV_GIB_PLAYER,
    EV_SCOREPLUM,
    EV_CTFMESSAGE,
    EV_BODYFADE,
    EV_SIEGE_ROUNDOVER,
    EV_SIEGE_OBJECTIVECOMPLETE,
    EV_DESTROY_GHOUL2_INSTANCE,
    EV_DESTROY_WEAPON_MODEL,
    EV_GIVE_NEW_RANK,
    EV_SET_FREE_SABER,
    EV_SET_FORCE_DISABLE,
    EV_WEAPON_CHARGE,
    EV_WEAPON_CHARGE_ALT,
    EV_SHIELD_HIT,
    EV_DEBUG_LINE,
    EV_TESTLINE,
    EV_STOPLOOPINGSOUND,
    EV_STARTLOOPINGSOUND,
    EV_TAUNT,
    EV_ANGER1,
    EV_ANGER2,
    EV_ANGER3,
    EV_VICTORY1,
    EV_VICTORY2,
    EV_VICTORY3,
    EV_CONFUSE1,
    EV_CONFUSE2,
    EV_CONFUSE3,
    EV_PUSHED1,
    EV_PUSHED2,
    EV_PUSHED3,
    EV_CHOKE1,
    EV_CHOKE2,
    EV_CHOKE3,
    EV_FFWARN,
    EV_FFTURN,
    EV_CHASE1,
    EV_CHASE2,
    EV_CHASE3,
    EV_COVER1,
    EV_COVER2,
    EV_COVER3,
    EV_COVER4,
    EV_COVER5,
    EV_DETECTED1,
    EV_DETECTED2,
    EV_DETECTED3,
    EV_DETECTED4,
    EV_DETECTED5,
    EV_LOST1,
    EV_OUTFLANK1,
    EV_OUTFLANK2,
    EV_ESCAPING1,
    EV_ESCAPING2,
    EV_ESCAPING3,
    EV_GIVEUP1,
    EV_GIVEUP2,
    EV_GIVEUP3,
    EV_GIVEUP4,
    EV_LOOK1,
    EV_LOOK2,
    EV_SIGHT1,
    EV_SIGHT2,
    EV_SIGHT3,
    EV_SOUND1,
    EV_SOUND2,
    EV_SOUND3,
    EV_SUSPICIOUS1,
    EV_SUSPICIOUS2,
    EV_SUSPICIOUS3,
    EV_SUSPICIOUS4,
    EV_SUSPICIOUS5,
    EV_COMBAT1,
    EV_COMBAT2,
    EV_COMBAT3,
    EV_JDETECTED1,
    EV_JDETECTED2,
    EV_JDETECTED3,
    EV_TAUNT1,
    EV_TAUNT2,
    EV_TAUNT3,
    EV_JCHASE1,
    EV_JCHASE2,
    EV_JCHASE3,
    EV_JLOST1,
    EV_JLOST2,
    EV_JLOST3,
    EV_DEFLECT1,
    EV_DEFLECT2,
    EV_DEFLECT3,
    EV_GLOAT1,
    EV_GLOAT2,
    EV_GLOAT3,
    EV_PUSHFAIL,
    EV_SIEGESPEC,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_26_event_values_match_openjk_landmarks() {
        assert_eq!(EntityEvent::EV_SABER_ATTACK.as_i32(), 29);
        assert_eq!(EntityEvent::EV_VOICECMD_SOUND.as_i32(), 75);
        assert_eq!(EntityEvent::EV_MISSILE_HIT.as_i32(), 85);
        assert_eq!(EntityEvent::EV_OBITUARY.as_i32(), 93);
        assert_eq!(EntityEvent::EV_DESTROY_WEAPON_MODEL.as_i32(), 104);
        assert_eq!(EntityEvent::EV_SHIELD_HIT.as_i32(), 110);
        assert_eq!(EntityEvent::EV_SIEGESPEC.as_i32(), 191);
        assert_eq!(EntityEvent::COUNT, 192);
    }

    #[test]
    fn conversion_rejects_event_bits_and_out_of_range_values() {
        assert_eq!(
            EntityEvent::try_from(EntityEvent::EV_SABER_HIT.as_i32()),
            Ok(EntityEvent::EV_SABER_HIT)
        );
        assert!(EntityEvent::try_from(EntityEvent::EV_SABER_HIT.as_i32() | EV_EVENT_BIT1).is_err());
        assert!(EntityEvent::try_from(-1).is_err());
        assert!(EntityEvent::try_from(EntityEvent::COUNT as i32).is_err());
    }
}
