//! Host boundary for private NinjaLiveCore surface blood.
pub const BLOOD_SIM_IMAGES: usize = 3;
/// R8 density input, one field-sized texture per reused blood slot.
// Higher-resolution authored masks are sampled into the original Ninja field;
// floor simulation stays at 256^2, wall/ceiling fields use 512^2.
pub const BLOOD_INK_RES: u32 = 512;
/// Existing 15-brush Ninja field budget, shared by impact seeds and moving contacts.
pub const BLOOD_MAX_CONTACTS: usize = 15;
#[derive(Clone, Copy, Debug)]
pub struct BloodDesc {
    pub extent_cm: [f32; 3],
    pub coast_ticks: u16,
    pub dry_ticks: u16,
}
/// One floor deposit. The direction is an impact orientation in pool UV axes,
/// not a measured fluid velocity. Zero requests an unoriented splat.
#[derive(Clone, Copy, Debug)]
pub struct BloodSeed {
    pub point_size_cm: f32,
    pub variation: u64,
    pub direction_uv: [f32; 2],
    /// Non-horizontal splats need higher spatial fidelity than large floor pools.
    /// Rendering still samples Ninja's own density and uses the same solver.
    pub high_quality: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct BloodMotion {
    pub uv: [f32; 2],
    pub velocity_cm_s: [f32; 2],
}
/// A sub-rectangle of the R8 `BLOOD_INK_RES`² ink image, tightly packed
/// (`size[0]` bytes per row). Texels outside it keep their previous upload,
/// so the host must cover (zero) any rectangle it inked on an earlier tick.
#[derive(Clone, Copy, Debug)]
pub struct BloodInk<'a> {
    pub bytes: &'a [u8],
    pub origin: [u32; 2],
    pub size: [u32; 2],
}
pub struct BloodOutputs<'a> {
    pub velocity_density: Option<&'a wgpu::TextureView>,
}
