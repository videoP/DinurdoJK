//! Static OpenJK cgame sound tables: `CG_RegisterWeapon` weapon sounds and the
//! `cgs.media` sound handles referenced by `CG_EntityEvent`.
//!
//! Pure data and lookups only, so event workers can build sound requests
//! without touching the VFS; the owner thread stages and decodes the assets.

pub const WP_STUN_BATON: i32 = 1;
pub const WP_MELEE: i32 = 2;
pub const WP_SABER: i32 = 3;
pub const WP_BRYAR_PISTOL: i32 = 4;
pub const WP_BLASTER: i32 = 5;
pub const WP_DISRUPTOR: i32 = 6;
pub const WP_BOWCASTER: i32 = 7;
pub const WP_REPEATER: i32 = 8;
pub const WP_DEMP2: i32 = 9;
pub const WP_FLECHETTE: i32 = 10;
pub const WP_ROCKET_LAUNCHER: i32 = 11;
pub const WP_THERMAL: i32 = 12;
pub const WP_TRIP_MINE: i32 = 13;
pub const WP_DET_PACK: i32 = 14;
pub const WP_CONCUSSION: i32 = 15;
pub const WP_BRYAR_OLD: i32 = 16;

// entChannel_t
pub const CHAN_AUTO: i32 = 0;
pub const CHAN_LOCAL: i32 = 1;
pub const CHAN_WEAPON: i32 = 2;
pub const CHAN_VOICE: i32 = 3;
pub const CHAN_BODY: i32 = 6;
pub const CHAN_AMBIENT: i32 = 7;
pub const CHAN_ANNOUNCER: i32 = 9;
pub const CHAN_MENU1: i32 = 11;

// trackchan_t
pub const TRACK_CHANNEL_2: i32 = 52; // force speed
pub const TRACK_CHANNEL_3: i32 = 53; // force rage
pub const TRACK_CHANNEL_5: i32 = 55; // force sight

// cgs.media
pub const SELECT_SOUND: &str = "sound/weapons/change.wav";
pub const FALL_SOUND: &str = "sound/player/fallsplat.wav";
pub const LAND_SOUND: &str = "sound/player/land1.wav";
pub const OBJECT_HIT_SOUND: &str = "sound/movers/objects/objectHit.wav";
pub const ROLL_SOUND: &str = "sound/player/roll1.wav";
pub const GIB_SOUND: &str = "sound/player/gibsplt1.wav";
pub const TELE_IN_SOUND: &str = "sound/player/telein.wav";
pub const TELE_OUT_SOUND: &str = "sound/player/teleout.wav";
pub const RESPAWN_SOUND: &str = "sound/items/respawn1.wav";
pub const HOLOCRON_PICKUP_SOUND: &str = "sound/player/holocron.wav";
pub const ZOOM_START_SOUND: &str = "sound/interface/zoomstart.wav";
pub const ZOOM_END_SOUND: &str = "sound/interface/zoomend.wav";
pub const DISRUPTOR_ZOOM_START: &str = "sound/weapons/disruptor/zoomstart.wav";
pub const DISRUPTOR_ZOOM_END: &str = "sound/weapons/disruptor/zoomend.wav";
pub const DISRUPTOR_ZOOM_LOOP: &str = "sound/weapons/disruptor/zoomloop.wav";
pub const DEPLOY_SEEKER_SOUND: &str = "sound/chars/seeker/misc/hiss";
pub const MEDKIT_SOUND: &str = "sound/items/use_bacta.wav";
pub const TEAM_HEAL_SOUND: &str = "sound/weapons/force/teamheal.wav";
pub const TEAM_REGEN_SOUND: &str = "sound/weapons/force/teamforce.wav";
pub const DRAIN_SOUND: &str = "sound/weapons/force/drained.mp3";
pub const COUNT_FIGHT_SOUND: &str = "sound/chars/protocol/misc/40MOM038";
pub const JEDI_MASTER_SABER_ON: &str = "sound/weapons/saber/saberon.wav";
pub const HAPPY_MUSIC: &str = "music/goodsmall.mp3";
pub const DRAMATIC_FAILURE: &str = "music/badsmall.mp3";

/// The `cg_weapons[weapon]` sound fields CG_EntityEvent / CG_Missile read.
#[derive(Clone, Copy, Debug, Default)]
pub struct WeaponSounds {
    pub select: Option<&'static str>,
    pub flash: Option<&'static str>,
    pub alt_flash: Option<&'static str>,
    pub charge: Option<&'static str>,
    pub alt_charge: Option<&'static str>,
    pub missile: Option<&'static str>,
    pub alt_missile: Option<&'static str>,
}

const ROCKET_LOOP: &str = "sound/weapons/rocket/missleloop.wav";

/// `CG_RegisterWeapon`'s per-weapon sound assignments. Weapons the switch does
/// not name use the `default:` branch (rocklf1a flash), like OpenJK.
pub fn weapon_sounds(weapon: i32) -> WeaponSounds {
    match weapon {
        WP_STUN_BATON => WeaponSounds {
            flash: Some("sound/weapons/baton/fire.mp3"),
            alt_flash: Some("sound/weapons/baton/fire.mp3"),
            ..WeaponSounds::default()
        },
        WP_MELEE | WP_SABER => WeaponSounds::default(),
        WP_CONCUSSION => WeaponSounds {
            select: Some("sound/weapons/concussion/select.wav"),
            flash: Some("sound/weapons/concussion/fire"),
            alt_flash: Some("sound/weapons/concussion/alt_fire"),
            alt_charge: Some("sound/weapons/bryar/altcharge.wav"),
            ..WeaponSounds::default()
        },
        WP_BRYAR_PISTOL | WP_BRYAR_OLD => WeaponSounds {
            select: Some("sound/weapons/bryar/select.wav"),
            flash: Some("sound/weapons/bryar/fire.wav"),
            alt_flash: Some("sound/weapons/bryar/alt_fire.wav"),
            alt_charge: Some("sound/weapons/bryar/altcharge.wav"),
            ..WeaponSounds::default()
        },
        WP_BLASTER => WeaponSounds {
            select: Some("sound/weapons/blaster/select.wav"),
            flash: Some("sound/weapons/blaster/fire.wav"),
            alt_flash: Some("sound/weapons/blaster/alt_fire.wav"),
            ..WeaponSounds::default()
        },
        WP_DISRUPTOR => WeaponSounds {
            select: Some("sound/weapons/disruptor/select.wav"),
            flash: Some("sound/weapons/disruptor/fire.wav"),
            alt_flash: Some("sound/weapons/disruptor/alt_fire.wav"),
            alt_charge: Some("sound/weapons/disruptor/altCharge.wav"),
            ..WeaponSounds::default()
        },
        WP_BOWCASTER => WeaponSounds {
            select: Some("sound/weapons/bowcaster/select.wav"),
            flash: Some("sound/weapons/bowcaster/fire.wav"),
            alt_flash: Some("sound/weapons/bowcaster/fire.wav"),
            charge: Some("sound/weapons/bowcaster/altcharge.wav"),
            ..WeaponSounds::default()
        },
        WP_REPEATER => WeaponSounds {
            select: Some("sound/weapons/repeater/select.wav"),
            flash: Some("sound/weapons/repeater/fire.wav"),
            alt_flash: Some("sound/weapons/repeater/alt_fire.wav"),
            ..WeaponSounds::default()
        },
        WP_DEMP2 => WeaponSounds {
            select: Some("sound/weapons/demp2/select.wav"),
            flash: Some("sound/weapons/demp2/fire.wav"),
            alt_flash: Some("sound/weapons/demp2/altfire.wav"),
            alt_charge: Some("sound/weapons/demp2/altCharge.wav"),
            ..WeaponSounds::default()
        },
        WP_FLECHETTE => WeaponSounds {
            select: Some("sound/weapons/flechette/select.wav"),
            flash: Some("sound/weapons/flechette/fire.wav"),
            alt_flash: Some("sound/weapons/flechette/alt_fire.wav"),
            ..WeaponSounds::default()
        },
        WP_ROCKET_LAUNCHER => WeaponSounds {
            select: Some("sound/weapons/rocket/select.wav"),
            flash: Some("sound/weapons/rocket/fire.wav"),
            alt_flash: Some("sound/weapons/rocket/alt_fire.wav"),
            missile: Some(ROCKET_LOOP),
            alt_missile: Some(ROCKET_LOOP),
            ..WeaponSounds::default()
        },
        WP_THERMAL => WeaponSounds {
            select: Some("sound/weapons/thermal/select.wav"),
            flash: Some("sound/weapons/thermal/fire.wav"),
            alt_flash: Some("sound/weapons/thermal/fire.wav"),
            charge: Some("sound/weapons/thermal/charge.wav"),
            alt_charge: Some("sound/weapons/thermal/charge.wav"),
            ..WeaponSounds::default()
        },
        WP_TRIP_MINE => WeaponSounds {
            select: Some("sound/weapons/detpack/select.wav"),
            flash: Some("sound/weapons/laser_trap/fire.wav"),
            alt_flash: Some("sound/weapons/laser_trap/fire.wav"),
            ..WeaponSounds::default()
        },
        WP_DET_PACK => WeaponSounds {
            select: Some("sound/weapons/detpack/select.wav"),
            flash: Some("sound/weapons/detpack/fire.wav"),
            alt_flash: Some("sound/weapons/detpack/fire.wav"),
            ..WeaponSounds::default()
        },
        18 => WeaponSounds::default(), // WP_TURRET
        _ => WeaponSounds {
            flash: Some("sound/weapons/rocket/rocklf1a.wav"),
            ..WeaponSounds::default()
        },
    }
}

/// `holdable_t` values CG_UseItem plays a sound for.
pub const HI_SEEKER: i32 = 1;
pub const HI_MEDPAC: i32 = 3;
pub const HI_MEDPAC_BIG: i32 = 4;
pub const HI_BINOCULARS: i32 = 5;

/// Team-power / predefined sound names (`PDSOUND_*`, 1-based).
pub fn predefined_sound(parm: i32) -> Option<&'static str> {
    Some(match parm {
        1 => "sound/weapons/force/protecthit.mp3",
        2 => "sound/weapons/force/protect.mp3",
        3 => "sound/weapons/force/absorbhit.mp3",
        4 => "sound/weapons/force/absorb.mp3",
        5 => "sound/weapons/force/jump.mp3",
        6 => "sound/weapons/force/grip.mp3",
        _ => return None,
    })
}

/// `EV_GLOBAL_TEAM_SOUND` announcer lines (`global_team_sound_t`). `ysalimari`
/// selects the GT_CTY variants for the flag return/taken pairs. The capture
/// cases (0, 1) are commented out in OpenJK.
pub fn global_team_sound(parm: i32, ysalimari: bool) -> Option<&'static str> {
    Some(match (parm, ysalimari) {
        (2, false) => "sound/chars/protocol/misc/40MOM041", // GTS_RED_RETURN: blue flag returned
        (2, true) => "sound/chars/protocol/misc/40MOM049",
        (3, false) => "sound/chars/protocol/misc/40MOM042", // GTS_BLUE_RETURN
        (3, true) => "sound/chars/protocol/misc/40MOM050",
        (4, false) => "sound/chars/protocol/misc/40MOM040", // GTS_RED_TAKEN
        (4, true) => "sound/chars/protocol/misc/40MOM048",
        (5, false) => "sound/chars/protocol/misc/40MOM039", // GTS_BLUE_TAKEN
        (5, true) => "sound/chars/protocol/misc/40MOM047",
        (6, _) => "sound/chars/protocol/misc/40MOM044", // GTS_REDTEAM_SCORED
        (7, _) => "sound/chars/protocol/misc/40MOM043",
        (8, _) => "sound/chars/protocol/misc/40MOM046", // GTS_REDTEAM_TOOK_LEAD
        (9, _) => "sound/chars/protocol/misc/40MOM045",
        (10, _) => "sound/chars/protocol/misc/40MOM032", // GTS_TEAMS_ARE_TIED
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weapon_table_matches_register_weapon() {
        assert_eq!(weapon_sounds(WP_BLASTER).flash, Some("sound/weapons/blaster/fire.wav"));
        assert_eq!(weapon_sounds(WP_BOWCASTER).charge, Some("sound/weapons/bowcaster/altcharge.wav"));
        assert_eq!(weapon_sounds(WP_ROCKET_LAUNCHER).missile, Some(ROCKET_LOOP));
        assert_eq!(weapon_sounds(WP_SABER).select, None);
        assert_eq!(weapon_sounds(17).flash, Some("sound/weapons/rocket/rocklf1a.wav"));
        assert_eq!(weapon_sounds(18).flash, None);
    }

    #[test]
    fn team_sounds_pick_flag_or_ysalimari_lines() {
        assert_eq!(global_team_sound(2, false), Some("sound/chars/protocol/misc/40MOM041"));
        assert_eq!(global_team_sound(2, true), Some("sound/chars/protocol/misc/40MOM049"));
        assert_eq!(global_team_sound(0, false), None);
    }
}
