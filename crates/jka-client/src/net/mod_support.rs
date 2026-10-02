//! Server identity is separate from native backend support. Deriving this from
//! current configstrings avoids carrying flags across servers or map changes.
use jka_protocol::commands::info_value;

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
}
