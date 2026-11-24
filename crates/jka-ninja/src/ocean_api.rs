//! Private Ocean2 provider boundary. World coordinates are cm in the source
//! z-up frame; the render bridge is responsible for converting its axes/units.
use crate::RiverImage;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OceanDesc {
    pub resolution: [u32; 2],
    pub extent_cm: [f32; 2],
    pub max_tracks: u32,
}

/// Two shader-stage-specific classes of authored texture. The host must load
/// and validate all images; no generic replacement texture is allowed.
#[derive(Clone, Copy, Debug)]
pub struct OceanImages {
    pub simulation: [RiverImage; 4],
    pub material: [OceanMaterialImage; 4],
}

/// None means the export does not establish the UE import color space. The
/// host must resolve it before creating a texture, not use a guessed default.
#[derive(Clone, Copy, Debug)]
pub struct OceanMaterialImage {
    pub qpath: &'static str,
    pub size: [u32; 2],
    pub srgb: Option<bool>,
}

pub struct OceanFrame<'a> {
    pub game_delta: f64,
    /// Shared wake tracks (`crate::WakeTrack`), relative to the field centre.
    pub tracks: &'a [crate::WakeTrack],
    /// The field's own motion (cm/s, track frame). Tracked velocities are taken
    /// relative to it, as UE's unit positions in a pawn-attached sim: the
    /// owner's travel cancels and only its limbs churn the water. Zero for a
    /// world-fixed field.
    pub frame_velocity_cm_s: [f32; 3],
    /// Optional GPU contact flags, in the same order as tracks.
    pub contacts: Option<&'a wgpu::Buffer>,
    /// Host extension. RGBA16F at capture resolution: RG reserved/zero, B density,
    /// A reserved/zero. Generated in the same command encoder from the
    /// pre-limit FFT crest field. None is the saved-reference no-SDF path.
    pub crest_source: Option<&'a wgpu::Texture>,
}

pub struct OceanOutputs<'a> {
    pub velocity_density: Option<&'a wgpu::TextureView>,
    pub pressure_divergence: Option<&'a wgpu::TextureView>,
}
