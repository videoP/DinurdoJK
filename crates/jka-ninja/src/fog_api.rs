//! Host inputs and outputs for a private ground-aligned Fluid Ninja field.

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FogPreset {
    LowLying,
    Plastic,
    Scattered,
    Mist,
}

#[derive(Clone, Copy, Debug)]
pub struct FogDesc {
    pub cells_per_side: u32,
    pub width_cm: f32,
    pub height_cm: f32,
}

/// Placement-relative centimetres in the renderer's X/Y-up/Z axes.
#[derive(Clone, Copy, Debug)]
pub struct FogStroke {
    pub position_cm: [f32; 3],
    pub previous_cm: [f32; 3],
    pub velocity_cm_s: [f32; 3],
    pub previous_velocity_cm_s: [f32; 3],
    pub brush_scale: f32,
}

pub struct FogFrame<'a> {
    pub game_delta: f64,
    /// Pawn origin relative to the placement, in renderer-axis centimetres.
    pub origin_cm: [f32; 3],
    /// Position of the placement in renderer-axis centimetres.
    pub anchor_cm: [f32; 3],
    pub strokes: &'a [FogStroke],
}

pub struct FogOutputs<'a> {
    pub velocity_density: Option<&'a wgpu::TextureView>,
    pub origin_cm: [f32; 3],
}

pub const FOG_SIM_IMAGES: usize = 2;

#[derive(Clone, Copy)]
pub struct FogImage {
    pub qpath: &'static str,
    /// Width, height and depth. Zero depth denotes a 2D image.
    pub size: [u32; 3],
    pub srgb: bool,
}

pub struct FogImages {
    /// Painter noise and amplitude noise, in that order.
    pub simulation: [FogImage; FOG_SIM_IMAGES],
    pub height_detail: FogImage,
    pub noise: [FogImage; 2],
}

#[derive(Clone, Copy)]
pub struct FogMistImages {
    pub density_input: FogImage,
    pub velocity_input: FogImage,
    pub density_noise: FogImage,
    pub passive_density: FogImage,
    pub passive_velocity: FogImage,
    pub blue_noise: FogImage,
}

/// `fog_volume_wgsl()` defines `ninja_fog_density(world_cm, eye_cm, base_cm,
/// width_cm, height_cm, time, preset, sky_rgb, light_direction, extend,
/// world_ddx_cm, world_ddy_cm)`. The last two are the world offsets to the
/// neighbouring voxels, standing in for the pixel-shader gradients UE's
/// VoxelizePS gives texture sampling. The host supplies the local surface
/// height in `base_cm.y` and keeps the simulation's horizontal origin.
/// It returns the host's `NinjaFogSample(extinction, albedo, emission)` in
/// linear color and per-metre coefficients. The host binds `ninja_fog_vd`,
/// `ninja_fog_height`, `ninja_fog_noise1`, `ninja_fog_noise2`,
/// `ninja_fog_clamp`, and `ninja_fog_wrap`.
pub const FOG_VOLUME_CONTRACT: () = ();
