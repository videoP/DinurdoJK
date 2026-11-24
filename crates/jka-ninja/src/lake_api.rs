//! Public boundary for the Fluid Ninja lake wake field: a small player-following
//! dense-water window over flat (Source) lakes, driven by the shared actor wake
//! (`crate::WakeTrack`). Plain data and wgpu handles only.

/// Square window of `extent_cm` at `resolution` solver cells. Track positions
/// are relative to the window centre (UE-style cm, z up, z = 0 at the lake plane).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LakeDesc {
    pub resolution: u32,
    pub extent_cm: f32,
    pub max_tracks: u32,
}

pub struct LakeFrame<'a> {
    /// Game-time seconds since the previous call; the field steps at its own fixed rate.
    pub game_delta: f64,
    pub tracks: &'a [crate::WakeTrack],
    /// The window's own motion (cm/s, track frame); see `OceanFrame::frame_velocity_cm_s`.
    pub frame_velocity_cm_s: [f32; 3],
    /// GPU contact flags (u32 per track, 1 = touching the water), same order as tracks.
    pub contacts: Option<&'a wgpu::Buffer>,
}

pub struct LakeOutputs<'a> {
    /// RGBA16F: velocity RG, density B, wetness A.
    pub velocity_density: Option<&'a wgpu::TextureView>,
    /// RG16F: pressure R, divergence G.
    pub pressure_divergence: Option<&'a wgpu::TextureView>,
}

/// WGSL contract for `lake_wake_wgsl()`: defines
/// `fn ninja_lake_wake(vd: texture_2d<f32>, pd: texture_2d<f32>, flow: texture_2d<f32>,
/// clamp_sampler: sampler, wrap_sampler: sampler, uv: vec2<f32>, world_cm: vec2<f32>,
/// time: f32) -> vec4<f32>` returning (tangent-normal tilt along window U, along
/// window V, foam coverage 0..1, 0). `uv` is the window UV (U = +x, V = +y in the track frame),
/// `world_cm` the absolute horizontal position in the same cm frame.
/// The same WGSL also defines `fn ninja_lake_caustics(caustics: texture_3d<f32>,
/// wrap_sampler: sampler, vd: texture_2d<f32>, pd: texture_2d<f32>, clamp_sampler: sampler,
/// sim_uv: vec2<f32>, sim_on: bool, bed_cm: vec2<f32>, behind_cm: f32, eye_distance_cm: f32,
/// time: f32, gx: vec2<f32>, gy: vec2<f32>) -> vec3<f32>`: the colour scale (1 = none) for the
/// lake bed seen through the water, from the caustics volume (`river_caustics_image`, W wraps)
/// at the bed position (cm, same frame as `world_cm`), the water path `behind_cm`, and the
/// screen derivatives of the position (cm). `sim_on`/`sim_uv` sample the wake window there.
pub const LAKE_WAKE_CONTRACT: () = ();
