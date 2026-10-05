//! Settings rendering.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextureFilter {
    Nearest,
    Bilinear,
    #[default]
    Trilinear,
    Anisotropic2x,
    Anisotropic4x,
    Anisotropic8x,
    Anisotropic16x,
}

impl TextureFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nearest => "NEAREST",
            Self::Bilinear => "BILINEAR",
            Self::Trilinear => "TRILINEAR",
            Self::Anisotropic2x => "ANISOTROPIC 2X",
            Self::Anisotropic4x => "ANISOTROPIC 4X",
            Self::Anisotropic8x => "ANISOTROPIC 8X",
            Self::Anisotropic16x => "ANISOTROPIC 16X",
        }
    }

    pub fn anisotropy_clamp(self) -> u16 {
        match self {
            Self::Anisotropic2x => 2,
            Self::Anisotropic4x => 4,
            Self::Anisotropic8x => 8,
            Self::Anisotropic16x => 16,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DetailTextureMode {
    #[default]
    Off,
    /// Mid-grey-neutral 2x modulation. The detail sample is converted back to
    /// its encoded (legacy fixed-function-like) value before modulation so 128
    /// grey is approximately a no-op.
    Neutral2x,
    /// Direct linear-space equivalent of blendFunc GL_DST_COLOR GL_SRC_COLOR.
    Linear2x,
    /// Previous test path: blendFunc GL_DST_COLOR GL_ONE.
    DstColorOne,
    /// Plain multiplicative modulation: blendFunc GL_DST_COLOR GL_ZERO.
    Multiply,
}

impl DetailTextureMode {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Neutral2x,
        Self::Linear2x,
        Self::DstColorOne,
        Self::Multiply,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Neutral2x => "NEUTRAL 2X",
            Self::Linear2x => "LINEAR 2X",
            Self::DstColorOne => "DST COLOR + ONE",
            Self::Multiply => "MULTIPLY",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Neutral2x => "neutral2x",
            Self::Linear2x => "linear2x",
            Self::DstColorOne => "dstcolor_one",
            Self::Multiply => "multiply",
        }
    }

    /// Numeric value sent directly to the WGSL override constant.
    pub fn shader_mode(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Neutral2x => 1,
            Self::Linear2x => 2,
            Self::DstColorOne => 3,
            Self::Multiply => 4,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" => Some(Self::Off),
            // Preserve configs written by the first detail-texture patch.
            "1" | "on" | "true" | "enhanced" | "neutral2x" | "softlight" => Some(Self::Neutral2x),
            "2" | "linear2x" | "modulate2x" => Some(Self::Linear2x),
            "3" | "dstcolor_one" | "dstcolorone" => Some(Self::DstColorOne),
            "4" | "multiply" | "modulate" => Some(Self::Multiply),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PvsMode {
    Off,
    Minimal,
    Full,
    #[default]
    Auto,
}

impl PvsMode {
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Full => "full",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FxGeometryMode {
    /// Current reference path: expand view-facing FX geometry on the presentation thread.
    #[default]
    Cpu,
    /// Preserve the exact same geometry/material semantics while parallelizing
    /// per-material FX expansion over the persistent Rayon worker pool.
    CpuWorkers,
    /// Keep OpenJK FX simulation on CPU, but submit billboard Particle sprites
    /// as compact WGPU instances instead of rebuilding quad vertices/indices.
    Gpu,
}

impl FxGeometryMode {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::CpuWorkers, Self::Gpu];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::CpuWorkers => "CPU WORKERS",
            Self::Gpu => "GPU",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CpuWorkers => "workers",
            Self::Gpu => "gpu",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" | "0" => Some(Self::Cpu),
            "workers" | "cpu_workers" | "cpu-workers" | "1" => Some(Self::CpuWorkers),
            "gpu" | "2" => Some(Self::Gpu),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Ghoul2SkinningMode {
    /// Faithful scalar CPU reference path.
    Cpu,
    /// Same OpenJK deformation math, batched over a persistent Rayon worker pool.
    CpuWorkers,
    /// CPU pose/bolt evaluation with GLM vertex deformation in the WGPU vertex shader.
    #[default]
    Gpu,
}

impl Ghoul2SkinningMode {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::CpuWorkers, Self::Gpu];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::CpuWorkers => "CPU WORKERS",
            Self::Gpu => "GPU",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CpuWorkers => "workers",
            Self::Gpu => "gpu",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" | "0" => Some(Self::Cpu),
            "workers" | "cpu_workers" | "cpu-workers" | "1" => Some(Self::CpuWorkers),
            "gpu" | "2" => Some(Self::Gpu),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Ghoul2BatchMode {
    Off,
    #[default]
    Adaptive,
    Force,
}

impl Ghoul2BatchMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Adaptive, Self::Force];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Adaptive => "ADAPTIVE",
            Self::Force => "FORCE",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "0",
            Self::Adaptive => "1",
            Self::Force => "2",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" => Some(Self::Off),
            "1" | "adaptive" | "auto" | "on" | "true" => Some(Self::Adaptive),
            "2" | "force" | "forced" => Some(Self::Force),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CullDebugMode {
    #[default]
    Off,
    RejectionReasons,
}

impl CullDebugMode {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlanarReflectionDebugMode {
    #[default]
    Off,
    Candidates,
    SelectedPlane,
    ReflectionTexture,
    AppliedSample,
    BindingTest,
}

impl PlanarReflectionDebugMode {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Candidates,
        Self::SelectedPlane,
        Self::ReflectionTexture,
        Self::AppliedSample,
        Self::BindingTest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Candidates => "CANDIDATES",
            Self::SelectedPlane => "SELECTED PLANE",
            Self::ReflectionTexture => "REFLECTION TEXTURE",
            Self::AppliedSample => "APPLIED SAMPLE",
            Self::BindingTest => "BINDING TEST",
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Candidates => 1.0,
            Self::SelectedPlane => 2.0,
            Self::ReflectionTexture => 3.0,
            Self::AppliedSample => 4.0,
            Self::BindingTest => 5.0,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => Some(Self::Off),
            "candidates" | "candidate" | "1" => Some(Self::Candidates),
            "selected" | "plane" | "selected_plane" | "2" => Some(Self::SelectedPlane),
            "texture" | "reflection_texture" | "3" => Some(Self::ReflectionTexture),
            "applied" | "sample" | "applied_sample" | "4" => Some(Self::AppliedSample),
            "binding" | "binding_test" | "source_test" | "5" => Some(Self::BindingTest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorLutPreset {
    #[default]
    Off,
    KodakVision3_250d,
    KodakPortra400,
    FujiEterna500,
    FujiVelvia50,
    /// Index into the `.cube` files found in `<base>/LUTs` (see `color_lut`).
    External(u16),
}

impl ColorLutPreset {
    pub(in crate::ui) const BUILT_IN: [Self; 5] = [
        Self::Off,
        Self::KodakVision3_250d,
        Self::KodakPortra400,
        Self::FujiEterna500,
        Self::FujiVelvia50,
    ];

    /// Built-in looks followed by every external `.cube` file.
    pub fn all() -> Vec<Self> {
        Self::BUILT_IN
            .into_iter()
            .chain((0..crate::color_lut::external_count() as u16).map(Self::External))
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::KodakVision3_250d => "KODAK VISION3 250D",
            Self::KodakPortra400 => "KODAK PORTRA 400",
            Self::FujiEterna500 => "FUJI ETERNA 500",
            Self::FujiVelvia50 => "FUJI VELVIA 50",
            Self::External(index) => crate::color_lut::external_label(index),
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::KodakVision3_250d => "kodak_vision3_250d",
            Self::KodakPortra400 => "kodak_portra_400",
            Self::FujiEterna500 => "fuji_eterna_500",
            Self::FujiVelvia50 => "fuji_velvia_50",
            Self::External(index) => crate::color_lut::external_key(index),
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(Self::Off),
            "kodak_vision3_250d" | "vision3_250d" | "kodak250d" => Some(Self::KodakVision3_250d),
            "kodak_portra_400" | "portra_400" | "portra400" => Some(Self::KodakPortra400),
            "fuji_eterna_500" | "eterna_500" | "eterna500" => Some(Self::FujiEterna500),
            "fuji_velvia_50" | "velvia_50" | "velvia50" => Some(Self::FujiVelvia50),
            other => crate::color_lut::find_external(other).map(Self::External),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EntityAmbientLightingMode {
    #[default]
    Off,
    BspLightgridClassic,
    BevyIrradianceVolume,
}

impl EntityAmbientLightingMode {
    pub const ALL: [Self; 3] = [
        Self::Off,
        Self::BspLightgridClassic,
        Self::BevyIrradianceVolume,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::BspLightgridClassic => "BSP LIGHTGRID (CLASSIC)",
            Self::BevyIrradianceVolume => "BEVY IRRADIANCE VOLUME (AMBIENT CUBES)",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::BspLightgridClassic => "bsp_lightgrid",
            Self::BevyIrradianceVolume => "bevy_irradiance_volume",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "bsp_lightgrid" => Some(Self::BspLightgridClassic),
            "bevy_irradiance_volume" => Some(Self::BevyIrradianceVolume),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DynamicLightsMode {
    #[default]
    Off,
    Legacy,
    Vertex,
    ClusteredLite,
    PerPixelForwardPlus,
    RayTracedHardware,
}

impl DynamicLightsMode {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Vertex,
        Self::Legacy,
        Self::ClusteredLite,
        Self::PerPixelForwardPlus,
        Self::RayTracedHardware,
    ];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Vertex => "vertex",
            Self::ClusteredLite => "clustered_lite",
            Self::PerPixelForwardPlus => "forward_plus",
            Self::RayTracedHardware => "ray_traced",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "legacy" => Some(Self::Legacy),
            "vertex" => Some(Self::Vertex),
            "clustered_lite" => Some(Self::ClusteredLite),
            "forward_plus" => Some(Self::PerPixelForwardPlus),
            "ray_traced" => Some(Self::RayTracedHardware),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SunVisibilityMode {
    Legacy,
    #[default]
    SkyPortals,
    Filtered,
}

impl SunVisibilityMode {
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::SkyPortals => "sky",
            Self::Filtered => "filtered",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "legacy" | "off" | "0" => Some(Self::Legacy),
            "sky" | "sky_portals" | "1" => Some(Self::SkyPortals),
            "filtered" | "high" | "2" => Some(Self::Filtered),
            _ => None,
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Legacy => 0.0,
            Self::SkyPortals => 1.0,
            Self::Filtered => 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DynamicShadowsMode {
    #[default]
    Off,
    Blob,
    /// Entity-only sun shadow map aimed by the baked lightgrid (players, NPCs, models).
    EntityMap,
    /// Bevy-style directional-light cascades (the only CSM implementation).
    CascadedShadowMaps,
    RayTraced,
}

impl DynamicShadowsMode {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Blob,
        Self::EntityMap,
        Self::CascadedShadowMaps,
        Self::RayTraced,
    ];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Blob => "blob",
            Self::EntityMap => "entity",
            Self::CascadedShadowMaps => "csm",
            Self::RayTraced => "ray_traced",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => Some(Self::Off),
            // `blob_stencil` was the old combined placeholder. Preserve old
            // configs by migrating it to the now-functional OpenJK blob mode.
            "blob" | "blob_stencil" | "1" => Some(Self::Blob),
            // `stencil` was a never-implemented placeholder for this slot; keep old
            // configs on the nearest functional entity-shadow technique.
            "entity" | "entity_map" | "stencil" | "stencil_legacy" => Some(Self::EntityMap),
            // Keep the old numeric aliases stable so archived configs do not
            // silently shift when EntityMap gets its own menu slot.
            // The old non-Bevy CSM was removed; its aliases land on the Bevy CSM.
            "csm"
            | "cascaded"
            | "cascaded_shadow_maps"
            | "2"
            | "csm_bevy"
            | "bevy_csm"
            | "cascaded_shadow_maps_bevy" => Some(Self::CascadedShadowMaps),
            "ray_traced" | "raytraced" | "3" | "4" => Some(Self::RayTraced),
            _ => None,
        }
    }
}

/// Where the Entity map shadow mode aims its light.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EntityShadowLight {
    /// The BSP lightgrid's baked dominant direction at the local player.
    #[default]
    Lightgrid,
    /// The strongest authored map light in range, taken from its real position
    /// (falls back to the lightgrid direction when no light qualifies).
    Authored,
}

impl EntityShadowLight {
    pub const ALL: [Self; 2] = [Self::Lightgrid, Self::Authored];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Lightgrid => "lightgrid",
            Self::Authored => "authored",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "lightgrid" | "grid" | "0" => Some(Self::Lightgrid),
            "authored" | "light" | "lights" | "1" => Some(Self::Authored),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DofQuality {
    Performance,
    #[default]
    Adaptive,
    High,
}

impl DofQuality {
    pub const ALL: [Self; 3] = [Self::Performance, Self::Adaptive, Self::High];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Adaptive => "adaptive",
            Self::High => "high",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "performance" | "perf" | "low" | "0" => Some(Self::Performance),
            "adaptive" | "auto" | "1" => Some(Self::Adaptive),
            "high" | "2" => Some(Self::High),
            _ => None,
        }
    }

    pub fn shader_value(self) -> f32 {
        match self {
            Self::Performance => 0.0,
            Self::Adaptive => 1.0,
            Self::High => 2.0,
        }
    }
}

pub const MAX_DISTANCE_CULL_SCALE: f32 = 10.0;

pub const VIDEO_ROW_FULLSCREEN: usize = 0;

pub const VIDEO_ROW_RESOLUTION: usize = 1;

pub const VIDEO_ROW_VSYNC: usize = 2;

pub const VIDEO_ROW_FPS_CAP: usize = 3;

pub const VIDEO_ROW_PHYSICS_FPS: usize = 4;

pub const VIDEO_ROW_BRIGHTNESS: usize = 5;

pub const VIDEO_ROW_ANTI_ALIASING: usize = 6;

pub const VIDEO_ROW_TEXTURE_FILTER: usize = 7;

pub const VIDEO_ROW_PVS: usize = 8;

pub const VIDEO_ROW_DISTANCE_CULL: usize = 9;

pub const VIDEO_ROW_GPU_DRIVEN: usize = 10;

pub const VIDEO_ROW_HIZ: usize = 11;

pub const VIDEO_ROW_ENTITY_AMBIENT_LIGHTING: usize = 12;

pub const VIDEO_ROW_DYNAMIC_LIGHTS: usize = 13;

pub const VIDEO_ROW_EMISSIVE_AREA_LIGHTS: usize = 14;

pub const VIDEO_ROW_AMBIENT_OCCLUSION: usize = 15;

pub const VIDEO_ROW_VOXEL_PROBE_GI: usize = 16;

pub const VIDEO_ROW_DYNAMIC_SHADOWS: usize = 17;

pub const VIDEO_ROW_LOCAL_LIGHT_SHADOWS: usize = 18;

pub const VIDEO_ROW_CONTACT_SHADOWS: usize = 19;

pub const VIDEO_ROW_SSR: usize = 20;

pub const VIDEO_ROW_PLANAR_REFLECTIONS: usize = 21;

pub const VIDEO_ROW_PLANAR_REFLECTION_DEBUG: usize = 22;

pub const VIDEO_ROW_HDR: usize = 23;

pub const VIDEO_ROW_TONE_MAPPING: usize = 24;

pub const VIDEO_ROW_BLOOM: usize = 25;

pub const VIDEO_ROW_MOTION_BLUR: usize = 26;

pub const VIDEO_ROW_DEPTH_OF_FIELD: usize = 27;

pub const VIDEO_ROW_DOF_QUALITY: usize = 28;

pub const VIDEO_ROW_HALATION: usize = 29;

pub const VIDEO_ROW_CHROMATIC_ABERRATION: usize = 30;

pub const VIDEO_ROW_VIGNETTE: usize = 31;

pub const VIDEO_ROW_COLOR_LUT: usize = 33;

pub const VIDEO_ROW_LUT_STRENGTH: usize = 34;

pub const VIDEO_ROW_WIREFRAME: usize = 35;

pub const VIDEO_ROW_CULL_DEBUG: usize = 36;

pub const VIDEO_ROW_DRAW_FPS: usize = 37;

pub const VIDEO_ROW_DEVELOPER_TOOLS: usize = 38;

pub const VIDEO_ROW_PERF_TRACE: usize = 39;

pub const VIDEO_ROW_GPU_TIMINGS: usize = 40;

pub const VIDEO_ROW_RENDER_BACKEND: usize = 41;

pub const VIDEO_ROW_BAKED_AO_SAMPLES: usize = 42;

pub const VIDEO_ROW_BAKED_AO_RESOLUTION: usize = 43;

pub const VIDEO_ROW_BAKED_AO_STRENGTH: usize = 44;

pub const VIDEO_ROW_BAKED_AO_RANGE: usize = 45;

pub const VIDEO_ROW_BAKED_AO_CURRENT_CELL: usize = 46;

pub const VIDEO_ROW_VID_RESTART: usize = 47;

pub const VIDEO_ROW_PBR: usize = 48;

pub const VIDEO_ROW_QUALITY_PRESET: usize = 49;

pub const VIDEO_ROW_AUTO_EXPOSURE: usize = 50;

pub const VIDEO_ROW_FLOAT_LIGHTMAP: usize = 51;

pub const VIDEO_ROW_GEN_NORMAL_MAPS: usize = 52;

pub const VIDEO_ROW_DELUXE_MAPPING: usize = 53;

pub const VIDEO_ROW_DELUXE_SPECULAR: usize = 54;

pub const VIDEO_ROW_ASSET_OVERRIDES: usize = 55;

pub const VIDEO_ROW_GHOUL2_SKINNING: usize = 56;

pub const VIDEO_ROW_GHOUL2_LOD_BIAS: usize = 57;

pub const VIDEO_ROW_GHOUL2_BATCH_DRAWS: usize = 58;

pub const VIDEO_ROW_GHOUL2_EARLY_CULL: usize = 59;

pub const VIDEO_ROW_MODERN_SABERS: usize = 60;

pub const VIDEO_ROW_FX_FPS: usize = 61;

pub const VIDEO_ROW_WORLD_LIGHTING: usize = 62;

pub const VIDEO_ROW_VERTEX_LIGHTING: usize = 63;

pub const VIDEO_ROW_LIGHTMAP_ONLY: usize = 64;

pub const VIDEO_ROW_MAP_LIGHT_SIMULATION: usize = 65;

pub const VIDEO_ROW_SABER_MARKS: usize = 66;

pub const VIDEO_ROW_MAX_FRAME_LATENCY: usize = 67;

pub const VIDEO_ROW_DETAIL_TEXTURES: usize = 68;

pub const VIDEO_ROW_FLARES: usize = 69;

pub const VIDEO_ROW_SABER_IMPACT_FX: usize = 70;

pub const VIDEO_ROW_FX_GEOMETRY: usize = 71;

pub const VIDEO_ROW_FX_ZERO_ALPHA_DISCARD: usize = 72;

pub const VIDEO_ROW_DRAW_MAP_MODELS: usize = 73;

pub const VIDEO_ROW_MODEL_BRIGHTNESS: usize = 74;

pub const VIDEO_ROW_DLIGHT_BRIGHTNESS: usize = 75;

pub const VIDEO_ROW_DRAW_TRIGGERS: usize = 76;

pub const VIDEO_ROW_DRAW_CLIP_BRUSHES: usize = 77;

pub const VIDEO_ROW_FX_PHYSICS: usize = 78;

pub const VIDEO_ROW_FX_LOD: usize = 79;

pub const VIDEO_ROW_ENTITY_SHADOW_LIGHT: usize = 80;

pub const VIDEO_ROW_DRAW_ENTITIES: usize = 81;

pub const VIDEO_ROW_RENDERER_VERBOSE: usize = 82;

pub const VIDEO_ROW_PICMIP: usize = 83;

pub const VIDEO_ROW_FX_FPS_SCOPE: usize = 84;

pub const VIDEO_ROW_DOF_AUTOFOCUS: usize = 85;

pub const ENV_ROW_WEATHER_WIND_SPEED: usize = 8;

pub const ENV_ROW_WEATHER_WIND_DIRECTION: usize = 9;

pub const ENV_ROW_GRASS: usize = 17;

pub const ENV_ROW_OCEAN: usize = 18;

pub const ENV_ROW_OCEAN_SETTINGS: usize = 19;

pub const ENV_ROW_VID_RESTART: usize = 20;

pub const ENV_ROW_SUN_SOURCE: usize = 21;

pub const ENV_ROW_SUN_YAW: usize = 22;

pub const ENV_ROW_SUN_PITCH: usize = 23;

pub const ENV_ROW_SUN_INTENSITY: usize = 24;

pub const ENV_ROW_SUN_COLOR: usize = 25;

pub const ENV_ROW_SUN_VISIBILITY: usize = 26;

pub const ENV_ROW_WEATHER_GUST_STRENGTH: usize = 27;

pub const ENV_ROW_WEATHER_DIRECTION_VARIATION: usize = 28;

pub const ENV_ROW_ENTITY_SUN_LIGHTING: usize = 32;

pub const SUN_INTENSITY_MAX: f32 = 4000.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectionQuality {
    /// No enhanced reflections and no authored tcGen environment stages.
    Off,
    /// Vanilla/authored tcGen environment stages and JKA portal mirrors only;
    /// no promoted environment-planar reflection path.
    Legacy,
    Low,
    Medium,
    #[default]
    High,
    Ultra,
}

impl ReflectionQuality {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Legacy,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Ultra,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Legacy => "LEGACY",
            Self::Low => "LOW",
            Self::Medium => "MEDIUM",
            Self::High => "HIGH",
            Self::Ultra => "ULTRA",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" => Some(Self::Off),
            "legacy" | "classic" => Some(Self::Legacy),
            "low" | "1" => Some(Self::Low),
            "medium" | "med" | "2" => Some(Self::Medium),
            "high" | "3" => Some(Self::High),
            "ultra" | "4" => Some(Self::Ultra),
            _ => None,
        }
    }

    /// Runtime reflection-technique tier. Off is the explicit compatibility-breaking
    /// "strip reflections" mode. Legacy preserves authored JKA reflection behavior
    /// (tcGen environment stages and portal mirrors) without promoting arbitrary
    /// environment-mapped surfaces into enhanced planar reflections.
    pub fn shader_value(self) -> u32 {
        match self {
            Self::Off | Self::Legacy => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::Ultra => 4,
        }
    }

    pub fn omits_environment_stages(self) -> bool {
        matches!(self, Self::Off)
    }

    pub fn ssr_enabled(self) -> bool {
        self >= Self::Medium
    }

    pub fn planar_slot_budget(self) -> usize {
        match self {
            Self::Off => 0,
            // Authored JKA `portal` mirrors are part of the legacy material
            // contract, not an enhanced reflection feature. Keep one baseline
            // planar slot available from Legacy upward.
            Self::Legacy | Self::Low | Self::Medium | Self::High => 1,
            Self::Ultra => 4,
        }
    }

    pub fn promotes_environment_planars(self) -> bool {
        matches!(self, Self::High | Self::Ultra)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlanarReflectionMode {
    Off,
    #[default]
    Authored,
    Environment,
}

impl PlanarReflectionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Authored => "AUTHORED MIRRORS",
            Self::Environment => "AUTHORED + ENVIRONMENT",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RendererBackend {
    #[default]
    Vulkan,
    Dx12,
}

impl RendererBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vulkan => "VULKAN",
            Self::Dx12 => "DX12",
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Vulkan => "vulkan",
            Self::Dx12 => "dx12",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "vulkan" | "vk" => Some(Self::Vulkan),
            "dx12" | "d3d12" | "directx12" => Some(Self::Dx12),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VsyncMode {
    #[default]
    Off,
    On,
    Fast,
    Adaptive,
}

impl VsyncMode {
    pub const ALL: [Self; 4] = [Self::Off, Self::On, Self::Fast, Self::Adaptive];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::On => "ON",
            Self::Fast => "FAST",
            Self::Adaptive => "ADAPTIVE",
        }
    }

    pub fn config_value(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::On => 1,
            Self::Fast => 2,
            Self::Adaptive => 3,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "off" => Some(Self::Off),
            "1" | "true" | "on" | "fifo" => Some(Self::On),
            "2" | "fast" | "mailbox" => Some(Self::Fast),
            "3" | "adaptive" | "relaxed" | "fifo_relaxed" => Some(Self::Adaptive),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FullscreenMode {
    #[default]
    Windowed,
    Borderless,
    Exclusive,
}

impl FullscreenMode {
    pub fn config_value(self) -> u8 {
        match self {
            Self::Windowed => 0,
            Self::Borderless => 1,
            Self::Exclusive => 2,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "false" | "off" | "windowed" => Some(Self::Windowed),
            "1" | "true" | "on" | "borderless" => Some(Self::Borderless),
            "2" | "exclusive" => Some(Self::Exclusive),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Windowed => "WINDOWED",
            Self::Borderless => "BORDERLESS",
            Self::Exclusive => "EXCLUSIVE",
        }
    }

    pub fn is_windowed(self) -> bool {
        matches!(self, Self::Windowed)
    }
}
