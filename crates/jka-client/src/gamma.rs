//! Where the master brightness curve is applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GammaMethod {
    #[default]
    Shader,
    Baked,
    Hardware,
}
impl GammaMethod {
    pub const ALL: [Self; 3] = [Self::Shader, Self::Baked, Self::Hardware];
    pub fn label(self) -> &'static str {
        match self {
            Self::Shader => "Shader",
            Self::Baked => "Baked textures",
            Self::Hardware => "Hardware",
        }
    }
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Shader => "shader",
            Self::Baked => "baked",
            Self::Hardware => "hardware",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "shader" | "software" => Some(Self::Shader),
            "1" | "baked" | "textures" => Some(Self::Baked),
            "2" | "hardware" => Some(Self::Hardware),
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
        assert_eq!(GammaMethod::parse("nope"), None);
    }
}
