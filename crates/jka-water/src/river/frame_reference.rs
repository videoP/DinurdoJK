//! CPU reference of `river_gpu::Simulation::encode_reference_frame`'s stage
//! order: PreSim history, source/sink (6), optional host CFS inlet,
//! distribute overlap (7), substeps of stages 8..12 and match spline,
//! then presentation (13) and normals/reset (14). It composes the existing
//! per-stage CPU references; it is the test oracle for sparse storage, not a
//! replacement solver. Dense and tiled storage run identical arithmetic.

use super::boundary::{self, Controls, SourceBox, WaterInfo};
use super::sparse::TileInit;
use super::stages::{self, DepthStencil, VelocityStencil};
use super::transport::{Field, Storage};
use super::{Geometry, Parameters, Presentation, Transport, Water};

/// Host volumetric inlet (river_discharge.wgsl): Q*dt spread over the open
/// cells of a rectangle whose bed is below the inlet level.
#[derive(Clone, Copy, Debug)]
pub struct Inlet {
    /// Grid cells [x0, y0, x1, y1), exclusive maximum.
    pub rect: [i32; 4],
    pub level: f32,
    pub volume_per_frame: f32,
    pub cell_area: f32,
}

pub struct Inputs<'a> {
    pub storage: Storage<'a>,
    pub geometry: &'a [Geometry],
    pub water_info: &'a [WaterInfo],
    pub boxes: &'a [SourceBox],
    pub controls: Controls,
    pub transport: Transport,
    pub presentation: Presentation,
    pub inlet: Option<Inlet>,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub water: Vec<Water>,
    pub previous_bottom: Vec<f32>,
    pub surface: Vec<Water>,
    pub normals: Vec<[f32; 4]>,
    history_needs_snapshot: bool,
}

impl State {
    /// Niagara reset: zero grids and zero bottom history.
    pub fn reset(len: usize) -> Self {
        Self {
            water: vec![Water::default(); len],
            previous_bottom: vec![0.0; len],
            surface: vec![Water::default(); len],
            normals: vec![[0.0, 0.0, 1.0, 0.0]; len],
            history_needs_snapshot: false,
        }
    }

    /// Append freshly activated tile cells (in storage order) after at least
    /// one completed frame: dry, still water and history equal to the bed.
    pub fn append(&mut self, geometry: &[Geometry]) {
        for &g in geometry {
            self.water.push(TileInit::water(g));
            self.previous_bottom.push(TileInit::previous_bottom(g));
            self.surface.push(Water::default());
            self.normals.push([0.0, 0.0, 1.0, 0.0]);
        }
    }
}

fn each<T>(storage: Storage<'_>, f: impl Fn(usize, i32, i32) -> T) -> Vec<T> {
    (0..storage.len())
        .map(|i| {
            let [x, y] = storage.cell(i);
            f(i, x, y)
        })
        .collect()
}

fn inlet_cells(inputs: &Inputs<'_>, inlet: Inlet) -> u32 {
    let mut cells = 0;
    for y in inlet.rect[1]..inlet.rect[3] {
        for x in inlet.rect[0]..inlet.rect[2] {
            if let Some(i) = inputs.storage.index([x, y]) {
                let g = inputs.geometry[i];
                cells += u32::from(g.collider < 1.0 - 1e-5 && g.bottom < inlet.level);
            }
        }
    }
    cells
}

/// One solver frame. `engine_delta` is the sink/frame delta.
pub fn frame(
    inputs: &Inputs<'_>,
    state: &mut State,
    p: Parameters,
    substeps: u32,
    engine_delta: f32,
) -> Result<(), &'static str> {
    let s = inputs.storage;
    let n = s.len();
    if inputs.geometry.len() != n
        || inputs.water_info.len() != n
        || state.water.len() != n
        || state.previous_bottom.len() != n
    {
        return Err("river reference frame fields differ from storage");
    }
    let geo = inputs.geometry;
    if state.history_needs_snapshot {
        for (h, g) in state.previous_bottom.iter_mut().zip(geo) {
            *h = g.bottom;
        }
        state.history_needs_snapshot = false;
    }
    let mut controls = inputs.controls;
    controls.frame_dt = engine_delta;
    let dx = p.dx;
    let water = each(s, |i, x, y| {
        let position = [
            controls.grid_min[0] + (x as f32 + 0.5) * dx,
            controls.grid_min[1] + (y as f32 + 0.5) * dx,
            controls.grid_min[2],
        ];
        boundary::apply(state.water[i], geo[i], inputs.water_info[i], position, inputs.boxes, controls)
    });
    state.water = water;
    if let Some(inlet) = inputs.inlet {
        let cells = inlet_cells(inputs, inlet);
        if cells > 0 {
            for i in 0..n {
                let [x, y] = s.cell(i);
                let g = geo[i];
                if x >= inlet.rect[0] && y >= inlet.rect[1] && x < inlet.rect[2] && y < inlet.rect[3]
                    && g.collider < 1.0 - 1e-5 && g.bottom < inlet.level
                {
                    let w = &mut state.water[i];
                    w.depth += inlet.volume_per_frame / (cells as f32 * inlet.cell_area);
                    w.height = g.bottom + w.depth;
                }
            }
        }
    }
    state.water = each(s, |i, _, _| {
        let input = state.water[i];
        let depth = input.depth.max(0.0).min(p.max_depth);
        Water { height: geo[i].bottom + depth, depth, ..input }
    });
    let t = inputs.transport;
    for _ in 0..substeps {
        let next = {
            let f = Field::with_storage(s, &state.water, geo)?;
            each(s, |_, x, y| f.extrapolate_velocity(x, y, t))
        };
        state.water = next;
        let next = {
            let f = Field::with_storage(s, &state.water, geo)?;
            each(s, |i, x, y| f.advect(x, y, state.previous_bottom[i], p, t))
        };
        state.water = next;
        let next = {
            let f = Field::with_storage(s, &state.water, geo)?;
            each(s, |_, x, y| f.extrapolate_height(x, y, t))
        };
        state.water = next;
        let next = {
            let f = Field::with_storage(s, &state.water, geo)?;
            each(s, |_, x, y| {
                let water = f.water(x, y);
                let velocity = stages::velocity(
                    VelocityStencil {
                        water,
                        geometry: f.geometry(x, y),
                        left_water: f.water(x - 1, y),
                        left_geometry: f.geometry(x - 1, y),
                        down_water: f.water(x, y - 1),
                        down_geometry: f.geometry(x, y - 1),
                    },
                    p,
                );
                Water { velocity, ..water }
            })
        };
        state.water = next;
        let next = {
            let f = Field::with_storage(s, &state.water, geo)?;
            each(s, |_, x, y| {
                let d = |dx: i32, dy: i32| f.water(x + dx, y + dy).depth;
                stages::depth(
                    DepthStencil {
                        water: f.water(x, y),
                        geometry: f.geometry(x, y),
                        depth: [d(-1, 0), d(1, 0), d(0, -1), d(0, 1)],
                        depth_two: [d(-2, 0), d(2, 0), d(0, -2), d(0, 2)],
                        right_velocity_x: f.water(x + 1, y).velocity[0],
                        up_velocity_y: f.water(x, y + 1).velocity[1],
                    },
                    p,
                )
            })
        };
        state.water = next;
        let next = each(s, |i, _, _| {
            boundary::match_spline(state.water[i], geo[i], inputs.water_info[i], p.dt, controls)
        });
        state.water = next;
    }
    let (water, surface): (Vec<_>, Vec<_>) = {
        let f = Field::with_storage(s, &state.water, geo)?;
        each(s, |_, x, y| f.present(x, y, inputs.presentation)).into_iter().unzip()
    };
    state.water = water;
    state.surface = surface;
    let (water, normals): (Vec<_>, Vec<_>) = {
        let f = Field::with_storage(s, &state.water, geo)?;
        each(s, |_, x, y| f.normal_and_reset(x, y, dx)).into_iter().unzip()
    };
    state.water = water;
    state.normals = normals;
    state.history_needs_snapshot = true;
    Ok(())
}

/// Total water volume (sum of non-negative depth) times cell area.
pub fn volume(state: &State, geometry: &[Geometry], cell_area: f32) -> f64 {
    state
        .water
        .iter()
        .zip(geometry)
        .filter(|(_, g)| g.collider < 1.0 - 1e-5)
        .map(|(w, _)| f64::from(w.depth.max(0.0)))
        .sum::<f64>()
        * f64::from(cell_area)
}

#[cfg(test)]
#[path = "frame_reference_tests.rs"]
mod tests;
