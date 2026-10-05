//! Settings environment.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FogMode {
    #[default]
    Off,
    LegacyDrawFog1,
    LegacyDrawFog2,
    Volumetric,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CloudType {
    #[default]
    Cumulus,
    Stratus,
    Storm,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CloudRenderResolution {
    Full,
    ThreeQuarter,
    #[default]
    Half,
    Quarter,
}

impl CloudRenderResolution {
    pub const ALL: [Self; 4] = [Self::Full, Self::ThreeQuarter, Self::Half, Self::Quarter];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::ThreeQuarter => "75",
            Self::Half => "half",
            Self::Quarter => "quarter",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" | "1" | "100" => Some(Self::Full),
            "75" | "0.75" | "threequarter" | "three-quarter" => Some(Self::ThreeQuarter),
            "half" | "2" | "50" => Some(Self::Half),
            "quarter" | "4" | "25" => Some(Self::Quarter),
            _ => None,
        }
    }

    pub fn scale(self) -> f32 {
        match self {
            Self::Full => 1.0,
            Self::ThreeQuarter => 0.75,
            Self::Half => 0.5,
            Self::Quarter => 0.25,
        }
    }
}

impl CloudType {
    pub const ALL: [Self; 3] = [Self::Cumulus, Self::Stratus, Self::Storm];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Cumulus => "cumulus",
            Self::Stratus => "stratus",
            Self::Storm => "storm",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cumulus" | "0" => Some(Self::Cumulus),
            "stratus" | "1" => Some(Self::Stratus),
            "storm" | "2" | "cumulonimbus" => Some(Self::Storm),
            _ => None,
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Cumulus => 0.0,
            Self::Stratus => 1.0,
            Self::Storm => 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RainIntensity {
    Light,
    #[default]
    Rain,
    Heavy,
}

impl RainIntensity {
    pub const ALL: [Self; 3] = [Self::Light, Self::Rain, Self::Heavy];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Rain => "rain",
            Self::Heavy => "heavy",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" | "lightrain" | "0" => Some(Self::Light),
            "rain" | "normal" | "1" => Some(Self::Rain),
            "heavy" | "heavyrain" | "2" => Some(Self::Heavy),
            _ => None,
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Light => 0.35,
            Self::Rain => 0.68,
            Self::Heavy => 1.0,
        }
    }

    pub fn particle_count(self) -> u32 {
        // V6 particles are persistent GPU simulation records shared by the
        // compute update, falling-streak render, and impact/splash render.
        match self {
            Self::Light => 6_000,
            Self::Rain => 14_000,
            Self::Heavy => 28_000,
        }
    }

    pub fn splash_particle_count(self) -> u32 {
        match self {
            Self::Light => 1_500,
            Self::Rain => 3_500,
            Self::Heavy => 6_000,
        }
    }

    pub fn haze_strength(self) -> f32 {
        match self {
            Self::Light => 0.24,
            Self::Rain => 0.52,
            Self::Heavy => 0.90,
        }
    }
}

/// How puddles and wet ground are shaded. `High` reuses the GodotOcean water
/// response (roughness-aware Fresnel, GGX sun and light glints) and traces
/// longer, streaked screen-space reflections; `Standard` keeps the cheaper film
/// response. Either way the cost is only paid on wet pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PuddleQuality {
    Standard,
    #[default]
    High,
}

impl PuddleQuality {
    pub const ALL: [Self; 2] = [Self::Standard, Self::High];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::High => "high",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "standard" | "normal" | "low" | "0" => Some(Self::Standard),
            "high" | "ocean" | "1" => Some(Self::High),
            _ => None,
        }
    }

    pub fn is_high(self) -> bool {
        self == Self::High
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FootprintMode {
    Off,
    TwoD,
    #[default]
    ThreeD,
}

impl FootprintMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::TwoD, Self::ThreeD];
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::TwoD => "2d",
            Self::ThreeD => "3d",
        }
    }
    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" => Some(Self::Off),
            "2d" | "2" | "legacy" | "mark" | "decal" => Some(Self::TwoD),
            "3d" | "3" | "deform" | "deformation" | "true" | "on" | "1" => Some(Self::ThreeD),
            _ => None,
        }
    }
    pub fn shader_value(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::TwoD => 1,
            Self::ThreeD => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SaberMarkMode {
    Off,
    #[default]
    Legacy,
    Enhanced,
}

impl SaberMarkMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Legacy, Self::Enhanced];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Enhanced => "enhanced",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" => Some(Self::Off),
            "legacy" | "1" | "openjk" | "classic" | "on" | "true" => Some(Self::Legacy),
            "enhanced" | "2" | "modern" | "molten" => Some(Self::Enhanced),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Legacy => "Legacy (OpenJK)",
            Self::Enhanced => "Enhanced molten",
        }
    }
}

pub const MAX_FOG_STRENGTH: f32 = 10.0;

pub const CLOUD_HEIGHT_MIN: f32 = -8192.0;

pub const CLOUD_HEIGHT_MAX: f32 = 8192.0;

pub const CLOUD_THICKNESS_MIN: f32 = 256.0;

pub const CLOUD_THICKNESS_MAX: f32 = 4096.0;

pub const VIDEO_ROW_FILM_GRAIN: usize = 32;

pub const ENV_ROW_FOG_MODE: usize = 0;

pub const ENV_ROW_FOG_STRENGTH: usize = 1;

pub const ENV_ROW_CLOUDS: usize = 2;

pub const ENV_ROW_CLOUD_TYPE: usize = 3;

pub const ENV_ROW_CLOUD_QUALITY: usize = 4;

pub const ENV_ROW_CLOUD_COVERAGE: usize = 5;

pub const ENV_ROW_CLOUD_HEIGHT: usize = 6;

pub const ENV_ROW_CLOUD_THICKNESS: usize = 7;

pub const ENV_ROW_CLOUD_SHADOWS: usize = 10;

pub const ENV_ROW_CLOUD_RENDER_RESOLUTION: usize = 11;

pub const ENV_ROW_CLOUD_TEMPORAL: usize = 12;

pub const ENV_ROW_CLOUD_TUNING: usize = 13;

pub const ENV_ROW_RAIN: usize = 14;

pub const ENV_ROW_RAIN_INTENSITY: usize = 15;

pub const ENV_ROW_FOOTPRINTS: usize = 16;

pub const ENV_ROW_PUDDLE_WATER: usize = 29;

pub const ENV_ROW_PUDDLE_SCATTER: usize = 30;

pub const ENV_ROW_RAIN_GRADE: usize = 31;

pub const CLOUD_ROW_TEMPORAL_DEPTH_FIX: usize = 0;

pub const CLOUD_ROW_SHEAR: usize = 1;

pub const CLOUD_ROW_BASE_VARIATION: usize = 2;

pub const CLOUD_ROW_AERIAL: usize = 3;

pub const CLOUD_ROW_SKY_AMBIENT: usize = 4;

pub const CLOUD_ROW_HISTORY_BLEND: usize = 5;

pub const CLOUD_ROW_MOTION_REJECT: usize = 6;

pub const CLOUD_ROW_HISTORY_DEPTH_REJECT: usize = 7;

pub const CLOUD_ROW_THICKNESS_VARIATION: usize = 8;

pub const CLOUD_ROW_SIZE: usize = 9;

pub const CLOUD_ROW_SHAPE_EVOLUTION: usize = 11;

pub const CLOUD_ROW_TERRAIN_INTERACTION: usize = 12;

pub const CLOUD_ROW_EMPTY_SKIP: usize = 13;

impl FogMode {
    pub fn is_legacy(self) -> bool {
        matches!(self, Self::LegacyDrawFog1 | Self::LegacyDrawFog2)
    }

    /// `r_drawfog`: OpenJK's 0/1/2, plus 3 for DinurdoJK's volumetric fog.
    pub fn drawfog_value(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::LegacyDrawFog1 => 1,
            Self::LegacyDrawFog2 => 2,
            Self::Volumetric => 3,
        }
    }

    pub fn from_drawfog(value: &str) -> Option<Self> {
        match value.trim() {
            "0" => Some(Self::Off),
            "1" => Some(Self::LegacyDrawFog1),
            "2" => Some(Self::LegacyDrawFog2),
            "3" => Some(Self::Volumetric),
            _ => None,
        }
    }
}
