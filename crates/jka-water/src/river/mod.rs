//! Source-backed pieces of UE 5.8.1 WaterAdvanced's river simulation.
//!
//! These are ported hydraulic/output stages, not yet the complete river system.
//! Static capture reconstruction and hydraulic/output stages are present.
//! GPU water/velocity grids use the source's R32f hardware-filtered attributes.
//! Hydraulic controls/substep scheduling and native reset-only grid mesh are
//! present. Remaining initialization, map adaptation, foam and material
//! presentation are still required.
//! Reference: Grid2D_SW_River's cached Niagara GPU shader, stages 1/2 and 5..14;
//! regenerate with scripts/ue-water-inspect.py for hashes and serialized offsets.
//! The original algorithms are Copyright Epic Games, Inc. All Rights Reserved.

pub mod authoring;
pub mod boundary;
pub mod capture;
pub mod control;
pub mod grid_mesh;
pub mod material;
pub mod ocean;
pub mod stages;
pub mod surface;
pub mod transport;
pub mod water_body;
pub mod water_info;
pub mod wave_advection;

pub const SHALLOW_SHADER: &str = include_str!("shallow.wgsl");

/// Selected transport settings from the main river asset's cached shader.
#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub extrapolation_half_width: u32,
    pub extrapolate_into_dry: bool,
    pub advection_multiplier: f32,
    pub minimum_advection_depth: f32,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            extrapolation_half_width: 2,
            extrapolate_into_dry: false,
            advection_multiplier: 1.0,
            minimum_advection_depth: 0.001,
        }
    }
}

/// Render-field smoothing: C++ defaults plus the cached stage's method/border.
#[derive(Clone, Copy, Debug)]
pub struct Presentation {
    pub half_width: u32,
    pub height_cutoff: f32,
    pub smoothing_method: u32,
    pub border_width: u32,
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            half_width: 5,
            height_cutoff: 500.0,
            smoothing_method: 2,
            border_width: 2,
        }
    }
}

/// Use the game's existing 40 map units/metre convention at the adapter boundary.
/// The Unreal solver operates in centimetres; its friction formula divides by
/// 100 internally. Changing only gravity would not preserve physical behavior.
pub const CENTIMETRES_PER_MAP_UNIT: f32 = 2.5;

/// Packed like Unreal's baked simulation texture: height, depth, velocity X/Y.
/// Heights are relative to the simulation origin, in centimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Water {
    pub height: f32,
    pub depth: f32,
    pub velocity: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Geometry {
    pub bottom: f32,
    pub collider: f32,
    pub collision_velocity: [f32; 2],
}

/// Parameters for the cached river shader's selected variant. Friction 0.001
/// and dissipation 1-1e-5 are its stage-11 constants, not JAPRO's Chezy values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameters {
    pub dx: f32,
    pub dt: f32,
    pub gravity: f32,
    pub max_velocity: f32,
    pub friction: f32,
    pub velocity_dissipation: f32,
    pub max_depth: f32,
    pub integration_depth_max: f32,
    pub fade_start: f32,
    pub fade_speed: f32,
}

impl Parameters {
    /// UpdateSimAttributes computes this reciprocal in two steps, with the
    /// selected WorldScale of one. Keep that expression in the port.
    pub fn dt_over_dx(self) -> f32 {
        1.0 / (self.dx / self.dt)
    }

    /// `gravity`, `dt`, and limits are supplied by the emitter's initialization;
    /// callers must not infer them from JKA movement gravity or frame rate.
    pub fn cached_river_variant(
        dx: f32,
        dt: f32,
        gravity: f32,
        max_velocity: f32,
        max_depth: f32,
    ) -> Self {
        Self {
            dx,
            dt,
            gravity,
            max_velocity,
            max_depth,
            friction: 0.001,
            velocity_dissipation: 1.0 - 1e-5,
            integration_depth_max: 200.0,
            fade_start: 0.1,
            fade_speed: 0.1,
        }
    }
}
