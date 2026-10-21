//! Settings rendering.


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GlowMode {
    #[default]
    Off,
    /// Only shader stages explicitly marked `glow` by the JKA material.
    JkaAuthored,
    /// HDR/luminance-driven modern bloom from the final scene.
    ModernBloom,
    /// Stock-authored glow stages plus modern HDR/luminance bloom.
    JkaAndBloom,
}

impl GlowMode {
    pub const ALL: [Self; 4] = [
        Self::Off,
        Self::JkaAuthored,
        Self::ModernBloom,
        Self::JkaAndBloom,
    ];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::JkaAuthored => "authored",
            Self::ModernBloom => "bloom",
            Self::JkaAndBloom => "combined",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "authored" => Some(Self::JkaAuthored),
            "bloom" => Some(Self::ModernBloom),
            "combined" => Some(Self::JkaAndBloom),
            _ => None,
        }
    }

    pub const fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub const fn authored(self) -> bool {
        matches!(self, Self::JkaAuthored | Self::JkaAndBloom)
    }

}

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
pub enum InlineTransformMode {
    #[default]
    Cpu,
    CpuWorkers,
    Gpu,
}

impl InlineTransformMode {
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
pub enum CrtScope {
    #[default]
    Off,
    Scene,
    SceneHud,
    Everything,
}

impl CrtScope {
    pub const ALL: [Self; 4] = [Self::Off, Self::Scene, Self::SceneHud, Self::Everything];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Scene => "SCENE",
            Self::SceneHud => "SCENE + HUD",
            Self::Everything => "EVERYTHING",
        }
    }

    pub fn config_value(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Scene => 1,
            Self::SceneHud => 2,
            Self::Everything => 3,
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" => Some(Self::Off),
            "1" | "scene" | "game" => Some(Self::Scene),
            "2" | "scene+hud" | "scene_hud" | "hud" => Some(Self::SceneHud),
            "3" | "everything" | "all" | "true" | "on" => Some(Self::Everything),
            _ => None,
        }
    }

}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrtStyle {
    #[default]
    LottesFast,
    Lottes,
}

impl CrtStyle {
    pub const ALL: [Self; 2] = [Self::LottesFast, Self::Lottes];

    pub fn config_value(self) -> &'static str {
        match self {
            Self::LottesFast => "fast",
            Self::Lottes => "lottes",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "fast" | "lottes_fast" | "lottes-fast" => Some(Self::LottesFast),
            "1" | "lottes" | "full" | "original" => Some(Self::Lottes),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrtQuality {
    #[default]
    Fast,
    Quality,
}

impl CrtQuality {
    pub const ALL: [Self; 2] = [Self::Fast, Self::Quality];

    pub fn config_value(self) -> u8 {
        match self { Self::Fast => 0, Self::Quality => 1 }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "fast" | "2" | "2tap" => Some(Self::Fast),
            "1" | "quality" | "8" | "8tap" => Some(Self::Quality),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrtMask {
    None,
    #[default]
    GrilleLite,
    Grille,
    Shadow,
}

impl CrtMask {
    pub const ALL: [Self; 4] = [Self::None, Self::GrilleLite, Self::Grille, Self::Shadow];

    /// Matches crt-lottes-fast's runtime MASK values exactly.
    pub fn shader_value(self) -> u32 {
        match self { Self::None => 0, Self::GrilleLite => 1, Self::Grille => 2, Self::Shadow => 3 }
    }

    pub fn config_value(self) -> u8 { self.shader_value() as u8 }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "none" => Some(Self::None),
            "1" | "lite" | "grille_lite" | "grillelite" => Some(Self::GrilleLite),
            "2" | "grille" | "aperture" => Some(Self::Grille),
            "3" | "shadow" | "shadowmask" => Some(Self::Shadow),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrtReferenceResolution {
    #[default]
    P480,
    P600,
    P720,
    Native,
}

impl CrtReferenceResolution {
    pub const ALL: [Self; 4] = [Self::P480, Self::P600, Self::P720, Self::Native];

    pub fn config_value(self) -> u32 {
        match self { Self::P480 => 480, Self::P600 => 600, Self::P720 => 720, Self::Native => 0 }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "480" | "480p" => Some(Self::P480),
            "600" | "600p" => Some(Self::P600),
            "720" | "720p" => Some(Self::P720),
            "0" | "native" => Some(Self::Native),
            _ => None,
        }
    }

    pub fn height(self, native_height: u32) -> f32 {
        let native = native_height.max(1);
        match self {
            Self::P480 => native.min(480) as f32,
            Self::P600 => native.min(600) as f32,
            Self::P720 => native.min(720) as f32,
            Self::Native => native as f32,
        }
    }
}

/// Controls where the existing depth-of-field blur is actually evaluated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DofMode {
    #[default]
    Off,
    UnderwaterOnly,
    On,
}

impl DofMode {
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::UnderwaterOnly => "underwater",
            Self::On => "on",
        }
    }

    pub fn from_config(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "0" | "off" => Some(Self::Off),
            "1" | "underwater" | "underwateronly" | "underwater_only" | "underwater only" | "underwater-only" => Some(Self::UnderwaterOnly),
            "2" | "on" | "always" => Some(Self::On),
            _ => None,
        }
    }
}

#[cfg(test)]
mod dof_mode_tests {
    use super::DofMode;

    #[test]
    fn modes_accept_named_and_numeric_cvar_values() {
        assert_eq!(DofMode::from_config("off"), Some(DofMode::Off));
        assert_eq!(DofMode::from_config("0"), Some(DofMode::Off));
        assert_eq!(DofMode::from_config("UNDERWATER"), Some(DofMode::UnderwaterOnly));
        assert_eq!(DofMode::from_config("1"), Some(DofMode::UnderwaterOnly));
        assert_eq!(DofMode::from_config("on"), Some(DofMode::On));
        assert_eq!(DofMode::from_config("2"), Some(DofMode::On));
        assert_eq!(DofMode::from_config("3"), None);
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

pub const VIDEO_ROW_COMMAND_RATE: usize = 4;

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

pub const VIDEO_ROW_GLOW: usize = 25;

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

/// Added after the existing row IDs; never renumber saved UI row ids.
pub const VIDEO_ROW_GPU_ADAPTER: usize = 103;

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

pub const VIDEO_ROW_DOF_MODE: usize = 111;

pub const VIDEO_ROW_CRT_SCOPE: usize = 86;
pub const VIDEO_ROW_CRT_STYLE: usize = 95;
pub const VIDEO_ROW_CRT_QUALITY: usize = 87;
pub const VIDEO_ROW_CRT_REFERENCE: usize = 88;
pub const VIDEO_ROW_CRT_MASK: usize = 89;
pub const VIDEO_ROW_CRT_SCANLINE: usize = 90;
pub const VIDEO_ROW_CRT_SHARPNESS: usize = 91;
pub const VIDEO_ROW_CRT_MASK_INTENSITY: usize = 92;
pub const VIDEO_ROW_CRT_CURVATURE: usize = 93;
pub const VIDEO_ROW_CRT_CORNER: usize = 94;
pub const VIDEO_ROW_PROMOTE_FX_LIGHTS: usize = 96;
pub const VIDEO_ROW_DYNAMIC_LIGHT_DEBUG: usize = 97;
pub const VIDEO_ROW_PROMOTE_SHADER_LIGHTS: usize = 98;
pub const VIDEO_ROW_PROMOTE_SHADER_GLOW: usize = 99;
pub const VIDEO_ROW_PROMOTE_SHADER_ADDITIVE: usize = 100;
pub const VIDEO_ROW_PROMOTE_SHADER_EMISSIVE: usize = 101;
/// Separate from the standard RBSP flare switch (VIDEO_ROW_FLARES).
pub const VIDEO_ROW_SABER_FLARES: usize = 102;

/// r_ext_compress_textures; available under Display & Graphics / Image Quality.
pub const VIDEO_ROW_TEXTURE_COMPRESSION: usize = 104;

pub const VIDEO_ROW_PATCH_SUBDIVISIONS: usize = 105;
pub const VIDEO_ROW_PATCH_LOD_ERROR: usize = 106;
pub const VIDEO_ROW_PATCH_STITCHING: usize = 107;
pub const VIDEO_ROW_DRAW_CURVES: usize = 108;
pub const VIDEO_ROW_PATCH_STATS: usize = 109;
pub const VIDEO_ROW_MODEL_LOD_SCALE: usize = 110;

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
pub const ENV_ROW_BAKED_SUN_REMOVAL: usize = 33;

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

    /// Runtime reflection-technique tier. Off strips standalone authored environment
    /// stages, but preserves integral multistage effects needed for visible materials.
    /// Legacy preserves all authored JKA reflection behavior
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
    Off,
    On,
    #[default]
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
