//! jaPRO server-authored `restricts` bits (CS_SERVERINFO).
//!
//! The server advertises the mask; client-side rendering/commands must obey it.
//! Keep the parsing in one place so live configstring changes are picked up by
//! the HUD, command handlers, camera and strafe-trail presenter alike.

pub(in crate::app) const PLAYER_LABELS: i32 = 1 << 6; // 64
pub(in crate::app) const FLIPKICK: i32 = 1 << 7; // 128
pub(in crate::app) const STRAFE_TRAILS: i32 = 1 << 8; // 256
pub(in crate::app) const DO: i32 = 1 << 9; // 512
pub(in crate::app) const THIRD_PERSON_ANGLE: i32 = 1 << 10; // 1024

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct ServerRestrictions(i32);

impl ServerRestrictions {
    pub(in crate::app) fn from_server_info(serverinfo: &[u8]) -> Self {
        Self(
            jka_protocol::commands::info_value(serverinfo, b"restricts")
                .map_or(0, jka_protocol::commands::atoi),
        )
    }

    pub(in crate::app) fn contains(self, bit: i32) -> bool {
        self.0 & bit != 0
    }

    /// TaystJK cg_view.c: only a live jaPRO race player's moving, non-BOTCPM
    /// third-person angle is reset. Following spectators are exempt. This
    /// resets the actual client cvar, not only the displayed camera offset.
    pub(in crate::app) fn restricts_race_angle(
        self,
        japro: bool,
        racing: bool,
        spectator_or_following: bool,
        moving_horizontally: bool,
        botcpm: bool,
    ) -> bool {
        self.contains(THIRD_PERSON_ANGLE)
            && japro
            && racing
            && !spectator_or_following
            && moving_horizontally
            && !botcpm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serverinfo_mask_and_combined_bits() {
        let flags = ServerRestrictions::from_server_info(br"\gamename\jaPRO\restricts\1344");
        assert!(flags.contains(PLAYER_LABELS));
        assert!(flags.contains(STRAFE_TRAILS));
        assert!(flags.contains(THIRD_PERSON_ANGLE));
        assert!(!flags.contains(DO));
        assert!(!flags.contains(FLIPKICK));
        assert_eq!(ServerRestrictions::from_server_info(br"\restricts\0"), ServerRestrictions::default());
        assert_eq!(ServerRestrictions::from_server_info(br"\restricts\invalid"), ServerRestrictions::default());
    }

    #[test]
    fn race_camera_has_taystjk_exemptions() {
        let flags = ServerRestrictions(THIRD_PERSON_ANGLE);
        assert!(flags.restricts_race_angle(true, true, false, true, false));
        assert!(!flags.restricts_race_angle(false, true, false, true, false));
        assert!(!flags.restricts_race_angle(true, false, false, true, false));
        assert!(!flags.restricts_race_angle(true, true, true, true, false));
        assert!(!flags.restricts_race_angle(true, true, false, false, false));
        assert!(!flags.restricts_race_angle(true, true, false, true, true));
        assert!(!ServerRestrictions::default().restricts_race_angle(true, true, false, true, false));
    }
}
