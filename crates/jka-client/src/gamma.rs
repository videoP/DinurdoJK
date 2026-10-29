//! Where the master brightness curve is applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GammaMethod {
    #[default]
    Shader,
    Baked,
    Hardware,
    HardwareSceneOnly,
}
impl GammaMethod {
    pub const ALL: [Self; 4] = [Self::Shader, Self::Baked, Self::Hardware, Self::HardwareSceneOnly];
    pub fn is_hardware(self) -> bool {
        matches!(self, Self::Hardware | Self::HardwareSceneOnly)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Shader => "Shader",
            Self::Baked => "Baked textures",
            Self::Hardware => "Hardware",
            Self::HardwareSceneOnly => "Hardware (scene only)",
        }
    }
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Shader => "shader",
            Self::Baked => "baked",
            Self::Hardware => "hardware",
            Self::HardwareSceneOnly => "hardware_scene",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "shader" | "software" => Some(Self::Shader),
            "1" | "baked" | "textures" => Some(Self::Baked),
            "2" | "hardware" => Some(Self::Hardware),
            "3" | "hardware_scene" | "hardware-scene" | "hw_scene" => Some(Self::HardwareSceneOnly),
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn methods_round_trip_with_legacy_numeric_aliases() {
        for (index, method) in GammaMethod::ALL.into_iter().enumerate() {
            assert_eq!(GammaMethod::parse(method.config_value()), Some(method));
            assert_eq!(GammaMethod::parse(&index.to_string()), Some(method));
        }
        assert_eq!(GammaMethod::parse(" SOFTWARE "), Some(GammaMethod::Shader));
        assert_eq!(GammaMethod::parse("HW_SCENE"), Some(GammaMethod::HardwareSceneOnly));
        assert!(GammaMethod::HardwareSceneOnly.is_hardware());
        assert!(GammaMethod::Hardware.is_hardware());
        assert!(!GammaMethod::Shader.is_hardware());
        assert_eq!(GammaMethod::parse("nope"), None);
    }
}
