//! Server identity is separate from native backend support. Deriving this from
//! current configstrings avoids carrying flags across servers or map changes.
use jka_protocol::commands::{atoi, info_value};

pub const TAYSTJK_INFO_RGBSABERS: i32 = 1 << 0;
pub const TAYSTJK_INFO_BLACKSABERS: i32 = 1 << 1;
pub const TAYSTJK_INFO_FLIPKICK: i32 = 1 << 2;
pub const TAYSTJK_INFO_GRAPPLE: i32 = 1 << 3;
pub const TAYSTJK_INFO_FIXROLL_1: i32 = 1 << 4;
pub const TAYSTJK_INFO_FIXROLL_2: i32 = 1 << 5;
pub const TAYSTJK_INFO_FIXROLL_3: i32 = 1 << 6;
pub const TAYSTJK_INFO_MOVEMENT_MASK: i32 = TAYSTJK_INFO_FLIPKICK
    | TAYSTJK_INFO_GRAPPLE
    | TAYSTJK_INFO_FIXROLL_1
    | TAYSTJK_INFO_FIXROLL_2
    | TAYSTJK_INFO_FIXROLL_3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMod {
    Base,
    Japro,
    Japlus,
    OpenJkAlt,
    BaseEnhanced,
    Lugormod,
    Unknown,
}

impl ServerMod {
    pub fn detect(info: &[u8]) -> Self {
        let name = info_value(info, b"gamename").unwrap_or_default();
        let starts = |prefix: &[u8]| {
            name.get(..prefix.len())
                .is_some_and(|v| v.eq_ignore_ascii_case(prefix))
        };
        if starts(b"japro") {
            Self::Japro
        } else if starts(b"JA+") || starts(b"^4U^3A^5Galaxy") || starts(b"AbyssMod") {
            Self::Japlus
        } else if starts(b"smU") {
            Self::OpenJkAlt
        } else if starts(b"base_enhanced") || starts(b"base_entranced") {
            Self::BaseEnhanced
        } else if starts(b"basejk") {
            Self::Base
        } else if name.eq_ignore_ascii_case(b"Lugormod")
            || name.eq_ignore_ascii_case(b"^5L^7ugormod ^5v3")
        {
            Self::Lugormod
        } else {
            Self::Unknown
        }
    }

    /// jaPRO's cgame only honours `SABER_RGB` (`cp_sbRGB1/2` relayed as `c3/c4`)
    /// when `cgs.serverMod >= SVMOD_JAPLUS`, i.e. on JA+ and jaPRO servers.
    pub fn supports_rgb_sabers(self) -> bool {
        matches!(self, Self::Japro | Self::Japlus)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Base => "Base JKA",
            Self::Japro => "JAPRO",
            Self::Japlus => "JA+",
            Self::OpenJkAlt => "OpenJK Alt",
            Self::BaseEnhanced => "Base Enhanced",
            Self::Lugormod => "Lugormod",
            Self::Unknown => "Unknown",
        }
    }
}


/// TaystJK feature bits are capabilities, not a mod identity. Current TaystJK
/// parses this key for every server so Base/other mods can opt into individual
/// client fixes without pretending to be jaPRO.
pub fn taystjk_info(info: &[u8]) -> i32 {
    info_value(info, b"taystJKinfo").map_or(0, atoi)
}

pub fn supports_rgb_sabers(info: &[u8]) -> bool {
    ServerMod::detect(info).supports_rgb_sabers()
        || taystjk_info(info) & TAYSTJK_INFO_RGBSABERS != 0
}

pub fn server_info(configstrings: &std::collections::BTreeMap<u16, Vec<u8>>) -> &[u8] {
    configstrings.get(&0).map(Vec::as_slice).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detection_uses_gamename_and_accepts_reference_prefixes() {
        assert_eq!(
            ServerMod::detect(br"\gamename\JaPRO 1.4\jcinfo\123"),
            ServerMod::Japro
        );
        assert_eq!(
            ServerMod::detect(br"\gamename\basejka\hostname\japro"),
            ServerMod::Base
        );
        assert_eq!(ServerMod::detect(br"\gamename\JA+ Mod"), ServerMod::Japlus);
        assert_eq!(
            ServerMod::detect(br"\gamename\futuremod\jcinfo\123"),
            ServerMod::Unknown
        );
        assert_eq!(ServerMod::detect(b""), ServerMod::Unknown);
    }

    #[test]
    fn taystjk_feature_flags_are_global_capabilities() {
        let base = br"\gamename\basejka\taystJKinfo\1";
        assert_eq!(taystjk_info(base), TAYSTJK_INFO_RGBSABERS);
        assert!(supports_rgb_sabers(base));
        assert!(!supports_rgb_sabers(br"\gamename\basejka"));
        assert!(supports_rgb_sabers(br"\gamename\JA+ Mod"));
    }
}
