//! Private Fluid Ninja Coast (Water_Dense_Sea / NinjaLive_Water_100m) provider
//! boundary. World coordinates are UE cm, z up, in the frame where the host
//! water plane is the actor's RVT.ClampingValue (`COAST_CLAMP_CM`).

/// Fixed, world-anchored coastal patch. `centre_cm` is the patch centre in the
/// same UE-cm frame as track positions (used for world-space noise/phase).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoastDesc {
    /// Solver cells (UE reference 2048). Below the reference the solver keeps
    /// the reference's world-space step (FluidNinja ScaleInvariant).
    pub resolution: [u32; 2],
    /// RT_VelocityDensity export size per axis, capped at `resolution`
    /// (UE water actor asset: 512). 0 exports at the solver resolution.
    pub velocity_density_resolution: u32,
    pub extent_cm: [f32; 2],
    pub centre_cm: [f32; 2],
    pub max_tracks: u32,
}

/// Per-frame inputs; identical to Ocean2's (tracks relative to the patch centre).
pub type CoastFrame<'a> = crate::OceanFrame<'a>;

pub struct CoastOutputs<'a> {
    pub velocity_density: Option<&'a wgpu::TextureView>,
    pub pressure_divergence: Option<&'a wgpu::TextureView>,
    /// RT_Painter; alpha carries the exposed relative landscape height / 100.
    pub painter: Option<&'a wgpu::TextureView>,
    /// FieldCaptureGrid (LandscapeGradient.xy, LandscapeElevation, LandscapePeaks), debug only.
    pub capture: Option<&'a wgpu::TextureView>,
}
