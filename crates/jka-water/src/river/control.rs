//! River initialization and frame stepping traced through serialized Niagara
//! pins, rapid-iteration parameters and emitter execution-loop metadata.
//! See control-reference.json and provenance.json. Original algorithms:
//! Copyright Epic Games, Inc. All Rights Reserved.

use super::Parameters;

/// Grid2D_SetResolution, Max Axis variant. The shorter axis is padded and its
/// world extent changes; using the original unpadded box would stretch cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub dimensions: [u32; 2],
    pub world_size: [f32; 2],
    pub cell_size: [f32; 2],
}

impl Grid {
    pub fn max_axis(world_size: [f32; 2], maximum_axis: u32) -> Result<Self, &'static str> {
        if world_size.iter().any(|v| !v.is_finite() || *v <= 0.0) || maximum_axis > i32::MAX as u32
        {
            return Err("invalid river world extents or maximum-axis resolution");
        }
        let cell_size = world_size[0].max(world_size[1]) / maximum_axis.max(1) as f32;
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err("invalid river cell size");
        }
        let mut cells = world_size.map(|v| (v / cell_size).floor());
        if world_size[0] > world_size[1] && (cell_size * cells[1] - world_size[1]).abs() > 1e-8 {
            cells[1] += 1.0;
        }
        if world_size[0] < world_size[1] && (cell_size * cells[0] - world_size[0]).abs() > 1e-8 {
            cells[0] += 1.0;
        }
        // SetResolution's final max(NumCells * ResolutionMult, 1), with the
        // main river's serialized multiplier of 1. Preserve f32 rounding.
        let dimensions = cells.map(|v| v.max(1.0) as u32);
        let world_size = cells.map(|v| v * cell_size);
        let cell_size = std::array::from_fn(|i| world_size[i] / dimensions[i] as f32);
        if cell_size.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err("river grid extent underflow");
        }
        Ok(Self {
            dimensions,
            world_size,
            cell_size,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Controls {
    pub substeps: u32,
    pub speed: f32,
    pub gravity: f32,
    pub max_depth_multiplier: f32,
    pub max_velocity_multiplier: f32,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            // ShallowWaterRiverActor.h, ScratchModule_03.Gravity and the
            // selected UpdateSimAttributes version's rapid-iteration values.
            substeps: 10,
            speed: 10.0,
            gravity: 980.0,
            max_depth_multiplier: 1.0,
            max_velocity_multiplier: 1.0,
        }
    }
}

/// Control snapshot for one encoded frame. Engine frame delta is
/// separate because the selected source uses it for sinks, not hydraulic dt.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    parameters: Parameters,
    substeps: u32,
    engine_delta: f32,
}

impl Frame {
    pub fn new(grid: Grid, controls: Controls, engine_delta: f32) -> Result<Self, &'static str> {
        if controls.substeps == 0
            || controls.substeps > i32::MAX as u32
            || [
                grid.cell_size[0],
                grid.cell_size[1],
                engine_delta,
                controls.speed,
                controls.gravity,
                controls.max_depth_multiplier,
                controls.max_velocity_multiplier,
            ]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
            || (grid.cell_size[0] - grid.cell_size[1]).abs() > grid.cell_size[0] * 1e-6
        {
            return Err("invalid river stepping controls or nonsquare simulation cells");
        }
        // Main emitter Divide_Float.A is a saved constant, not Engine.DeltaTime.
        // Preserve its exact f32 bytes and the Min_Float.B rapid parameter.
        let input_delta = (0.016667f32 / controls.substeps as f32).min(0.011111f32);
        let dt = input_delta * controls.speed;
        let dx = grid.cell_size[0];
        // Selected module WorldScale is 1. This is already a cm-space solver;
        // the JKA-to-cm conversion belongs at the map adapter boundary.
        let dx_over_dt = dx / dt;
        let max_depth =
            ((dx_over_dt * dx_over_dt) / controls.gravity) * controls.max_depth_multiplier;
        let dt_over_dx = 1.0 / dx_over_dt;
        let max_velocity =
            ((max_depth * dt_over_dx) * controls.gravity) * controls.max_velocity_multiplier;
        if [dt, max_depth, max_velocity]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("river stepping controls overflow or underflow");
        }
        Ok(Self {
            parameters: Parameters::cached_river_variant(
                dx,
                dt,
                controls.gravity,
                max_velocity,
                max_depth,
            ),
            substeps: controls.substeps,
            engine_delta,
        })
    }

    pub fn parameters(self) -> Parameters {
        self.parameters
    }
    pub fn substeps(self) -> u32 {
        self.substeps
    }
    pub fn engine_delta(self) -> f32 {
        self.engine_delta
    }
    pub fn hydraulic_duration(self) -> f64 {
        f64::from(self.parameters.dt) * f64::from(self.substeps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn river_grid_pads_the_short_axis_instead_of_stretching_cells() {
        let g = Grid::max_axis([100.0, 37.0], 16).unwrap();
        assert_eq!(g.dimensions, [16, 6]);
        assert_eq!(g.world_size, [100.0, 37.5]);
        assert_eq!(g.cell_size, [6.25, 6.25]);
        assert_eq!(
            Grid::max_axis([37.0, 100.0], 16).unwrap().dimensions,
            [6, 16]
        );
        assert_eq!(Grid::max_axis([10.0, 10.0], 0).unwrap().dimensions, [1, 1]);
    }

    #[test]
    fn river_hydraulic_timing_is_independent_of_sink_frame_delta() {
        let grid = Grid::max_axis([256.0, 128.0], 16).unwrap();
        let a = Frame::new(grid, Controls::default(), 1.0 / 30.0).unwrap();
        let b = Frame::new(grid, Controls::default(), 1.0 / 240.0).unwrap();
        assert_eq!(a.parameters(), b.parameters());
        assert_ne!(a.engine_delta(), b.engine_delta());
        assert_eq!(a.parameters().dt, (0.016667f32 / 10.0) * 10.0);
        let one = Frame::new(
            grid,
            Controls {
                substeps: 1,
                speed: 1.0,
                ..Controls::default()
            },
            0.016,
        )
        .unwrap();
        assert_eq!(one.parameters().dt, 0.011111);
        assert!(a.hydraulic_duration() > 0.16);
    }

    #[test]
    fn river_controls_reject_invalid_or_overflowed_inputs() {
        let grid = Grid::max_axis([256.0, 128.0], 16).unwrap();
        assert!(Grid::max_axis([f32::INFINITY, 1.0], 128).is_err());
        assert!(Frame::new(
            grid,
            Controls {
                substeps: 0,
                ..Controls::default()
            },
            0.016
        )
        .is_err());
        assert!(Frame::new(
            grid,
            Controls {
                speed: 0.0,
                ..Controls::default()
            },
            0.016
        )
        .is_err());
        assert!(Frame::new(grid, Controls::default(), f32::NAN).is_err());
        assert!(Frame::new(
            grid,
            Controls {
                gravity: f32::MIN_POSITIVE,
                ..Controls::default()
            },
            0.016
        )
        .is_err());
    }
}
