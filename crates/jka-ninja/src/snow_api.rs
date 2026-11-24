//! Public contract for the optional Ninja snow painter and material.

pub const SNOW_MAX_CONTACTS: usize = 96;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SnowProfile {
    Minimal,
    Composite,
}

impl SnowProfile {
    pub const fn extent_metres(self) -> f32 {
        match self {
            Self::Minimal => 100.0,
            Self::Composite => 50.0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SnowImage {
    pub qpath: &'static str,
    pub size: [u32; 2],
    pub srgb: bool,
}

pub struct SnowMaterialShaders {
    /// Humpy1 material functions for the renderer's snow shell.
    pub humpy1: &'static str,
    pub composite: &'static str,
    /// SnowPatterns input material used by the full Snow demo.
    pub density_pattern: &'static str,
    /// Humpy1 tile maps 1/2 and dedicated roughness map, in that order.
    pub surface_images: [SnowImage; 3],
    /// Amplitude noise, density input, and velocity-only input.
    pub painter_images: [SnowImage; 3],
    pub density_image: SnowImage,
}

/// A traced ground source in UE-facing world centimetres (X/Y horizontal,
/// Z up). Previous position uses the same world origin, not the old window UV.
#[derive(Clone, Copy, Debug)]
pub struct SnowContact {
    pub position_cm: [f32; 3],
    pub previous_cm: [f32; 3],
    pub velocity_cm_s: [f32; 3],
    pub previous_velocity_cm_s: [f32; 3],
}
