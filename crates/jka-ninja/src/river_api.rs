//! Public boundary types for the river presentation field. Plain data and
//! wgpu handles only, so the private implementation and the stub share them.
//! No Fluid Ninja graph or asset knowledge belongs here.

/// One required input image (qpath and expected size) for the host loader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiverImage {
    pub qpath: &'static str,
    pub size: [u32; 2],
    /// Decode as sRGB (true) or linear data (false).
    pub srgb: bool,
}

/// Optional volume source packed as a row-major atlas of equal-sized slices.
pub struct RiverVolumeImage {
    pub image: RiverImage,
    pub slice_size: [u32; 2],
    /// Normalized separable filter in XY for the volume mip chain. W uses
    /// a pair average, with the final slice repeated when needed.
    pub mip_filter: [f32; 8],
}

/// Live controls for the private submerged post material. Colors are linear RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NinjaUnderwaterSettings {
    pub distance_mult: f32,
    pub distortion: f32,
    pub brightness: f32,
    pub caustics_mult: f32,
    pub distance_color: [f32; 3],
    pub caustics_color: [f32; 3],
}

impl Default for NinjaUnderwaterSettings {
    fn default() -> Self {
        crate::river_underwater_defaults()
    }
}

/// Field placement: same centre and world extent as the river's live grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiverDesc {
    /// Simulation resolution (independent of the hydraulic grid).
    pub resolution: [u32; 2],
    /// World extent of the domain in centimetres.
    pub extent_cm: [f32; 2],
}

/// Per-frame inputs, valid inside the current command encoder only.
pub struct RiverFrame<'a> {
    /// R32Float D2Array, 4 layers: height relative to origin (cm), depth (cm),
    /// velocity X (cm/s), velocity Y (cm/s). Hydraulic resolution.
    pub state: &'a wgpu::Texture,
    /// R32Float D2, hydraulic resolution, 1 = solid or dry, 0 = open water.
    pub solid_or_dry: &'a wgpu::Texture,
    /// GPU contact flags (u32 per track, 1 = native water contact admitted),
    /// written earlier in the same encoder. `None` = no tracked actors.
    pub contact_flags: Option<&'a wgpu::Buffer>,
    pub tracks: &'a [crate::WakeTrack],
    /// Game-time seconds since the previous frame (not wall time, not the
    /// accelerated hydraulic clock). The field steps at its own fixed rate.
    pub game_delta: f64,
    /// Views of the images listed by `river_input_images`, same order.
    pub images: [&'a wgpu::TextureView; 5],
}

/// Exported presentation textures (absent until the first step exports).
pub struct RiverOutputs<'a> {
    /// RGBA16F: velocity RG, density B, wetness A.
    pub velocity_density: Option<&'a wgpu::TextureView>,
    /// RGBA16F: brush velocity RG, paint density B, elevation A.
    pub painter: Option<&'a wgpu::TextureView>,
    /// RG16F: pressure R, divergence G.
    pub pressure: Option<&'a wgpu::TextureView>,
    /// RGBA16F: nearest-surface distance (cm) in R, field-capture resolution.
    pub distance: Option<&'a wgpu::TextureView>,
}

/// Internal wake probe inputs. The first 48 bytes of each hash stroke are
/// three float vec4s (position/size, previous/keep, velocity). They are
/// followed by radius (f32), admission (u32) and 8 bytes of padding.
pub struct RiverWakeDebug<'a> {
    pub hash_params: &'a wgpu::Buffer,
    pub strokes: &'a wgpu::Buffer,
    /// Count followed by the stored stroke IDs.
    pub global: &'a wgpu::Buffer,
    /// One count and a fixed-capacity list of IDs per coarse cell.
    pub neighbors: &'a wgpu::Buffer,
    pub painter_before: &'a wgpu::TextureView,
    pub painter_after: &'a wgpu::TextureView,
}

/// WGSL contract for `river_material_wgsl()`: the host shader declares
/// `struct RiverNinjaSurface { enabled: f32, normal: vec3<f32>, base_color: vec3<f32>,
/// roughness: f32, opacity: f32, specular: f32, refraction_ior: f32 }` and the bindings `ninja_vd`,
/// `ninja_pd`, `ninja_distance`, `ninja_flow_map`, `ninja_roughness_map` (2D textures),
/// `ninja_caustics` (3D texture), `ninja_clamp`, `ninja_wrap` (sampler). The material WGSL
/// defines `fn ninja_river_shade(uv: vec2<f32>, world: vec3<f32>, eye: vec3<f32>,
/// pixel_depth: f32, time: f32, two_sided_sign: f32) -> RiverNinjaSurface`
/// returning a tangent-space normal (z up on the water mesh).
/// The material reads the host's `authored.ninja_albedo.rgb` surface tint and
/// `authored.ninja_foam_color.rgb` foam tint. Their reset values are one.
/// `ninja_river_color_scale(uv, world, eye, extent, pixel_depth, scene_depth, time)`
/// returns the SLW color scale only when the volume atlas is bound.
/// `ninja_river_volume_coefficients(color_scale)` returns `RiverSlwCoefficients`.
/// A zero color scale selects the material's baseline without the optional atlas.
/// The underwater entry points use the scene reconstruction helpers and the
/// noise, gradient and volume bindings in the host's river_underwater.wgsl.
pub const RIVER_MATERIAL_CONTRACT: () = ();
