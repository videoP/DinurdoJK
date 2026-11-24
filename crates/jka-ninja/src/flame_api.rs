//! Host-side data for a private Fluid Ninja flame field.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlamePreset {
    Bonfire,
    BurningMan,
    Candle,
    Torch,
    Logfire,
    /// Fire_Smoke_Explosive Stage 11 "Flaming Fist" setup B (Pawn2, demo
    /// key 2): a blue smoke trail painted by the left middle finger.
    SmokingFist,
    /// Stage 14 "Burning Man 3" (Pawn6, demo key 3), surface flame layer
    /// (NinjaLive_Flames3).
    BurningMan3,
    /// Stage 14 smoke layer (NinjaLive_Smoke2): a separate, larger field
    /// presented as a heterogeneous volume. Runs beside `BurningMan3`.
    BurningMan3Smoke,
    /// Stage 15 "Smoking Man" (Pawn5, NinjaLive3): smoke only, presented as
    /// a heterogeneous volume.
    SmokingMan,
}

impl FlamePreset {
    pub const ALL: [Self; 9] = [
        Self::Bonfire,
        Self::BurningMan,
        Self::Candle,
        Self::Torch,
        Self::Logfire,
        Self::SmokingFist,
        Self::BurningMan3,
        Self::BurningMan3Smoke,
        Self::SmokingMan,
    ];

    /// Fields painted from the presented player's skeletal pose.
    pub const fn body_driven(self) -> bool {
        matches!(self, Self::BurningMan | Self::SmokingFist | Self::BurningMan3 | Self::BurningMan3Smoke | Self::SmokingMan)
    }
}

/// Same order on the host and in the reference's skeletal filter.
pub const FLAME_BODY_JOINTS: usize = 17;
/// Extra movement-only brushes, separate from the authored skeleton emitters.
pub const FLAME_MOTION_MAX_STROKES: usize = 32;
pub const FLAME_CAPTURE_DOWNSCALE: u32 = 4;

/// A physical contact in render-axis centimetres. The fluid painter uses its
/// motion but does not treat it as a density-emitting source.
#[derive(Clone, Copy, Debug)]
pub struct FlameMotionStroke {
    pub current_cm: [f32; 3],
    pub previous_cm: [f32; 3],
    /// Approximate half-strength radius of the fluid impulse, not FX glow.
    pub radius_cm: f32,
}

/// Placement is expressed in render axes, with +Y up, and in centimetres.
#[derive(Clone, Copy, Debug)]
pub struct FlameDesc {
    pub resolution: [u32; 2],
    pub extent_cm: [f32; 3],
    /// Offset from the spawn to the simulation centre. Burning Man uses the
    /// tracked component's forward, up, and right axes at placement.
    pub centre_offset_cm: [f32; 3],
    /// CamFaceBillboard selects world up while retaining the view's right axis.
    pub billboard_world_up: bool,
    /// The mesh-distance capture uses the preset's PainterEdgeMaskWidth.
    pub painter_edge_mask_width: f32,
    /// Present the field as a heterogeneous volume instead of the surface.
    pub volume: Option<FlameVolumeDesc>,
}

/// Niagara volume renderer box for a camera-facing field. Local X is the
/// field's U (view right), local Y its V (view down) and local Z points at
/// the camera, as CameraFacingFunction orients the component. X and Y span
/// `extent_cm[0..2]`; Z spans `depth_cm`. The box is centred on the field.
#[derive(Clone, Copy, Debug)]
pub struct FlameVolumeDesc {
    pub depth_cm: f32,
    /// VolumeResolutionMaxAxis: voxel count along the longest box axis. The
    /// host's ray-march step is one voxel (StepFactor 1).
    pub resolution_max_axis: u32,
}

pub struct FlameFrame<'a> {
    pub eye_cm: [f32; 3],
    pub centre_cm: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    /// Ambient wind in render axes, in centimetres per second.
    pub wind_cm_s: [f32; 3],
    /// Unjittered camera projection, taking positions in centimetres.
    pub world_to_clip_cm: [[f32; 4]; 4],
    pub game_delta: f64,
    pub body_joints_cm: Option<&'a [Option<[f32; 3]>; FLAME_BODY_JOINTS]>,
    /// External swept contacts (sabers); never add smoke density.
    pub motion_strokes: &'a [FlameMotionStroke],
    pub body_scale: f32,
    /// Burning Man skeletal brush strength over its ignition ramp (0..=1).
    pub body_fuel: f32,
    /// DinurdoJK option: ignore KillBrushBelowThisVelocity so a body-driven
    /// field keeps emitting at full strength while the player stands still.
    pub emit_at_rest: bool,
    /// Multiplier on how long the flame's smoke trail persists (density
    /// decay). 1 is the preset's authored persistence.
    pub smoke_lifetime: f32,
    pub world_brush_mask: Option<FlameBrushMask<'a>>,
}

/// MeshField capture with collision alpha in RGBA16F. RGB is zero for a
/// static BSP collision-only source. The host writes it on the GPU earlier
/// in the same encoder; the field copies it when `revision` changes.
pub struct FlameBrushMask<'a> {
    pub texture: &'a wgpu::Texture,
    pub revision: u64,
}

pub struct FlameOutputs<'a> {
    pub velocity_density: Option<&'a wgpu::TextureView>,
}

pub struct FlameImage {
    pub qpath: &'static str,
    pub size: [u32; 2],
    pub srgb: bool,
    pub mipmaps: bool,
}

/// Simulation image slots are painter noise, density noise, amplitude noise,
/// optional combined input bitmap and optional velocity-only input bitmap.
pub const FLAME_SIM_IMAGES: usize = 5;

pub struct FlameImages {
    pub simulation: [Option<FlameImage>; FLAME_SIM_IMAGES],
    pub detail: Option<FlameImage>,
    /// 3D noise for a volume material (`flame_volume_noise`).
    pub volume_noise: Option<crate::fog_api::FogImage>,
}

/// Local plane mesh with expanded triangle vertices as little-endian
/// `[plane_u, plane_v, tex_u, tex_v]` floats in the unit plane.
pub struct FlameMesh {
    pub qpath: &'static str,
    pub vertices: u32,
}

pub const FLAME_MESH_VERTEX_BYTES: u64 = 16;

/// `flame_material_wgsl()` defines `ninja_flame_shade(uv, time, depth_gap_cm)`
/// and `ninja_flame_offset_cm(uv, normal)`. The host provides
/// `flame_velocity_density`, `flame_detail`, `flame_clamp` and `flame_wrap`,
/// and sets `var<private> flame_view_cosine: f32` (the absolute cosine between
/// the plane normal and the direction to the eye) before each shade call.
/// The shade result is straight-alpha, scene-linear RGBA.
pub const FLAME_MATERIAL_CONTRACT: () = ();

/// `flame_volume_wgsl()` defines `ninja_flame_volume(uvw, time, sky_rgb)`,
/// returning the host's `NinjaFlameVolumeSample(extinction, emission,
/// albedo)`: per-centimetre extinction and emission, in scene-linear colour.
/// `uvw` is the box position in 0..1 (see `FlameVolumeDesc`). The host binds
/// `flame_velocity_density`, `flame_volume_noise` (3D), `flame_clamp` and
/// `flame_wrap`.
pub const FLAME_VOLUME_CONTRACT: () = ();
