//! Sparse-storage equivalence tests (long-river handoff tests A..E).
use super::*;
use crate::river::control::{Controls as Stepping, Frame, Grid};
use crate::river::sparse::{required_tiles, Lattice, Residency, TileInit};

const DX: f32 = 20.0;
const CFS_CM3: f32 = 28_316.846;

#[derive(Clone, Copy)]
enum Terrain {
    Straight,
    Curved,
    Branching,
    Basin,
    Ledge,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Level,
    Discharge,
}

fn bed(terrain: Terrain, x: i32, y: i32) -> f32 {
    let (xf, yf) = (x as f32, y as f32);
    let slope = 300.0 - 0.5 * xf;
    match terrain {
        Terrain::Basin => 50.0,
        Terrain::Ledge => if x >= 32 { 1000.0 } else { 50.0 },
        Terrain::Straight => slope + if (yf - 96.0).abs() > 6.0 { 40.0 } else { 0.0 },
        Terrain::Curved => {
            let centre = 96.0 + 30.0 * (xf / 25.0).sin();
            slope + if (yf - centre).abs() > 6.0 { 40.0 } else { 0.0 }
        }
        Terrain::Branching => {
            let main = xf < 110.0 && (yf - 96.0).abs() <= 6.0;
            let up = xf >= 110.0 && (yf - (96.0 + (xf - 110.0))).abs() <= 6.0;
            let down = xf >= 110.0 && (yf - (96.0 - (xf - 110.0))).abs() <= 6.0;
            let pool = (xf - 140.0).powi(2) + (yf - 96.0).powi(2) <= 144.0;
            let bank = !(main || up || down || pool);
            slope - if pool { 60.0 } else { 0.0 } + if bank { 40.0 } else { 0.0 }
        }
    }
}

/// Dense geometry with the capture's two-cell border colliders.
fn dense_geometry(terrain: Terrain, n: [usize; 2]) -> Vec<Geometry> {
    (0..n[0] * n[1])
        .map(|i| {
            let (x, y) = ((i % n[0]) as i32, (i / n[0]) as i32);
            let border = x < 2 || y < 2 || x > n[0] as i32 - 3 || y > n[1] as i32 - 3;
            Geometry {
                bottom: bed(terrain, x, y),
                collider: if border { 1.0 } else { 0.0 },
                collision_velocity: [0.0; 2],
            }
        })
        .collect()
}

fn stepping(n: [usize; 2]) -> (Parameters, u32) {
    let grid = Grid {
        dimensions: n.map(|v| v as u32),
        world_size: n.map(|v| v as f32 * DX),
        cell_size: [DX; 2],
    };
    // Unreal's default SimSpeed 10: the cached river variant's timestep.
    let frame = Frame::new(grid, Stepping::default(), 1.0 / 60.0).unwrap();
    (frame.parameters(), frame.substeps())
}

fn controls() -> Controls {
    let mut c = Controls::cached_river_variant(1.0 / 60.0, [0.0; 3]);
    c.match_spline = false; // simulation-first: no spline matching
    c
}

struct Source {
    boxes: Vec<SourceBox>,
    inlet: Option<Inlet>,
    cells: [i32; 4],
}

fn source(terrain: Terrain, mode: Mode, at: [i32; 2], frame_seconds: f32) -> Source {
    let target = bed(terrain, at[0], at[1]) + 60.0;
    let cells = [at[0] - 4, at[1] - 4, at[0] + 5, at[1] + 5];
    let level_box = SourceBox {
        position: [(at[0] as f32 + 0.5) * DX, (at[1] as f32 + 0.5) * DX, 0.0],
        size: [8.0 * DX, 8.0 * DX, 2.0 * target],
        angle: 0.0,
        sink: false,
    };
    match mode {
        Mode::Level => Source { boxes: vec![level_box], inlet: None, cells },
        Mode::Discharge => Source {
            boxes: Vec::new(),
            inlet: Some(Inlet {
                rect: cells,
                level: target,
                volume_per_frame: 50.0 * CFS_CM3 * frame_seconds,
                cell_area: DX * DX,
            }),
            cells,
        },
    }
}

fn bits(w: Water) -> [u32; 4] {
    [w.height.to_bits(), w.depth.to_bits(), w.velocity[0].to_bits(), w.velocity[1].to_bits()]
}

fn pristine(w: Water, g: Geometry) -> bool {
    w == TileInit::water(g)
}

struct Sparse {
    residency: Residency,
    geometry: Vec<Geometry>,
    info: Vec<WaterInfo>,
    state: State,
}

impl Sparse {
    fn new(lattice: Lattice) -> Self {
        Self {
            residency: Residency::new(lattice),
            geometry: Vec::new(),
            info: Vec::new(),
            state: State::default(),
        }
    }

    /// Make tiles resident; new cells take the dense bed. Before the first
    /// frame they take the Niagara reset, afterwards TileInit.
    fn activate(&mut self, tiles: &[[i32; 2]], dense: &[Geometry], width: usize, started: bool) -> usize {
        let mut added = 0;
        for &tile in tiles {
            let (_, new) = self.residency.insert(tile).unwrap();
            if !new {
                continue;
            }
            added += 1;
            let first = self.geometry.len();
            for i in first..self.residency.cell_count() {
                let [x, y] = self.residency.cell(i);
                self.geometry.push(dense[y as usize * width + x as usize]);
                self.info.push(WaterInfo::default());
            }
            if started {
                self.state.append(&self.geometry[first..]);
            } else {
                let reset = State::reset(self.residency.cell_count() - first);
                self.state.water.extend(reset.water);
                self.state.previous_bottom.extend(reset.previous_bottom);
                self.state.surface.extend(reset.surface);
                self.state.normals.extend(reset.normals);
            }
        }
        added
    }

    /// Tiles holding any cell that differs from a dry, still, untouched cell.
    fn occupied(&self) -> Vec<[i32; 2]> {
        let lattice = self.residency.lattice();
        let mut tiles: Vec<_> = (0..self.geometry.len())
            .filter(|&i| !pristine(self.state.water[i], self.geometry[i]))
            .map(|i| lattice.tile_of(self.residency.cell(i)))
            .collect();
        tiles.sort_unstable();
        tiles.dedup();
        tiles
    }
}

/// Dense reference against tiled storage. `expand` grows residency from the
/// source tiles with a one-tile guard ring; otherwise every tile is resident
/// from the start. Every resident cell must match bit for bit.
fn compare(terrain: Terrain, mode: Mode, tile: u32, tiles: [u32; 2], at: [i32; 2], frames: u32, expand: bool) -> (usize, usize) {
    let lattice = Lattice::new(tile, tiles).unwrap();
    let n = lattice.dimensions().map(|v| v as usize);
    let dense_geo = dense_geometry(terrain, n);
    let (p, substeps) = stepping(n);
    let src = source(terrain, mode, at, p.dt * substeps as f32);
    let dense_info = vec![WaterInfo::default(); dense_geo.len()];
    let mut dense = State::reset(dense_geo.len());
    let mut sparse = Sparse::new(lattice);
    let initial: Vec<_> = if expand {
        let lo = lattice.tile_of([src.cells[0], src.cells[1]]);
        let hi = lattice.tile_of([src.cells[2] - 1, src.cells[3] - 1]);
        let touched = (lo[1]..=hi[1]).flat_map(|y| (lo[0]..=hi[0]).map(move |x| [x, y]));
        required_tiles(lattice, touched, 1)
    } else {
        Residency::full(lattice).slot_tiles().to_vec()
    };
    sparse.activate(&initial, &dense_geo, n[0], false);
    let first_tiles = sparse.residency.slot_count();
    let transport = Transport::default();
    let presentation = Presentation::default();
    for _ in 0..frames {
        let dense_inputs = Inputs {
            storage: Storage::Dense { width: n[0], height: n[1] },
            geometry: &dense_geo,
            water_info: &dense_info,
            boxes: &src.boxes,
            controls: controls(),
            transport,
            presentation,
            inlet: src.inlet,
        };
        frame(&dense_inputs, &mut dense, p, substeps, 1.0 / 60.0).unwrap();
        let sparse_inputs = Inputs {
            storage: Storage::Tiled(&sparse.residency),
            geometry: &sparse.geometry,
            water_info: &sparse.info,
            boxes: &src.boxes,
            controls: controls(),
            transport,
            presentation,
            inlet: src.inlet,
        };
        frame(&sparse_inputs, &mut sparse.state, p, substeps, 1.0 / 60.0).unwrap();
        for i in 0..sparse.geometry.len() {
            let [x, y] = sparse.residency.cell(i);
            let d = y as usize * n[0] + x as usize;
            assert_eq!(bits(sparse.state.water[i]), bits(dense.water[d]), "water at {x},{y}");
            assert_eq!(bits(sparse.state.surface[i]), bits(dense.surface[d]), "surface at {x},{y}");
            assert_eq!(
                sparse.state.normals[i].map(f32::to_bits),
                dense.normals[d].map(f32::to_bits),
                "normal at {x},{y}"
            );
        }
        // Cells never made resident must still be untouched in the dense run.
        for (d, (w, g)) in dense.water.iter().zip(&dense_geo).enumerate() {
            let at = [(d % n[0]) as i32, (d / n[0]) as i32];
            if sparse.residency.index(at).is_none() {
                assert!(pristine(*w, *g), "dense water reached non-resident cell {at:?}");
            }
        }
        let dense_volume = volume(&dense, &dense_geo, DX * DX);
        let sparse_volume = volume(&sparse.state, &sparse.geometry, DX * DX);
        assert!((dense_volume - sparse_volume).abs() <= dense_volume.abs() * 1e-9);
        if expand {
            let occupied = sparse.occupied();
            let before = volume(&sparse.state, &sparse.geometry, DX * DX);
            sparse.activate(&required_tiles(lattice, occupied, 1), &dense_geo, n[0], true);
            // Activation never creates water.
            assert_eq!(volume(&sparse.state, &sparse.geometry, DX * DX), before);
        }
    }
    assert!(dense.water.iter().any(|w| w.depth > 1.0), "source produced no water");
    (first_tiles, sparse.residency.slot_count())
}

/// Handoff test A: one dense 256^2 grid against 4x4 resident tiles of 64.
#[test]
fn tiled_storage_is_bit_identical_to_dense_grid() {
    let (first, last) = compare(Terrain::Straight, Mode::Level, 64, [4, 4], [60, 60], 3, false);
    assert_eq!((first, last), (16, 16));
}

/// Handoff tests B/C/D/E: expanding residency stays bit-identical to the
/// dense grid along straight, curved and branching/pooling channels, with
/// volume parity and no water created by activation. Sources sit one cell
/// from a tile edge so the guard ring must grow on the first frame.
#[test]
fn expanding_tiles_match_dense_straight_channel() {
    let (first, last) = compare(Terrain::Straight, Mode::Level, 32, [6, 6], [91, 91], 12, true);
    assert!(last > first, "residency did not expand ({first} -> {last})");
}

#[test]
fn expanding_tiles_match_dense_curved_channel() {
    let (first, last) = compare(Terrain::Curved, Mode::Level, 32, [6, 6], [91, 91], 12, true);
    assert!(last > first);
}

#[test]
fn expanding_tiles_match_dense_branching_pool() {
    let (first, last) = compare(Terrain::Branching, Mode::Level, 32, [6, 6], [100, 96], 12, true);
    assert!(last > first);
}

/// The CFS (Discharge) inlet is preserved under expansion as well.
#[test]
fn expanding_tiles_match_dense_discharge_inlet() {
    let (first, last) = compare(Terrain::Straight, Mode::Discharge, 32, [6, 6], [91, 91], 12, true);
    assert!(last > first);
}

/// Backstop when the guard is exceeded: a non-resident tile reads as the
/// native wall. The hydraulic state next to it is bit-identical to the same
/// basin bounded by resident dry high ground, and the same water flows on
/// when the neighbour tile is resident at basin level.
#[test]
fn non_resident_neighbour_matches_dry_high_ground() {
    let lattice = Lattice::new(32, [2, 1]).unwrap();
    let n = lattice.dimensions().map(|v| v as usize);
    let basin = dense_geometry(Terrain::Basin, n);
    let ledge = dense_geometry(Terrain::Ledge, n);
    let (p, substeps) = stepping(n);
    let run = |geometry: &[Geometry], tiles: &[[i32; 2]]| {
        let mut sparse = Sparse::new(lattice);
        sparse.activate(tiles, geometry, n[0], true);
        for i in 0..sparse.geometry.len() {
            let [x, _] = sparse.residency.cell(i);
            let g = sparse.geometry[i];
            if x < 32 && g.collider < 0.5 {
                let depth = if x >= 24 { 120.0 } else { 100.0 };
                sparse.state.water[i] = Water { height: g.bottom + depth, depth, velocity: [0.0; 2] };
            }
        }
        for _ in 0..20 {
            let inputs = Inputs {
                storage: Storage::Tiled(&sparse.residency),
                geometry: &sparse.geometry,
                water_info: &sparse.info,
                boxes: &[],
                controls: controls(),
                transport: Transport::default(),
                presentation: Presentation::default(),
                inlet: None,
            };
            frame(&inputs, &mut sparse.state, p, substeps, 1.0 / 60.0).unwrap();
        }
        sparse
    };
    let walled = run(&basin, &[[0, 0]]);
    let high = run(&ledge, &[[0, 0], [1, 0]]);
    let mut wall_wet = false;
    for i in 0..walled.geometry.len() {
        let at = walled.residency.cell(i);
        let j = high.residency.index(at).unwrap();
        assert_eq!(bits(walled.state.water[i]), bits(high.state.water[j]), "water at {at:?}");
        assert_eq!(bits(walled.state.surface[i]), bits(high.state.surface[j]), "surface at {at:?}");
        wall_wet |= at[0] == 31 && walled.state.water[i].depth > 1.0;
    }
    assert!(wall_wet, "no water against the non-resident edge");
    let open = run(&basin, &[[0, 0], [1, 0]]);
    let crossed = (0..open.geometry.len())
        .filter(|&i| open.residency.cell(i)[0] >= 32)
        .any(|i| open.state.water[i].depth > 0.0);
    assert!(crossed, "water did not enter the resident neighbour tile");
}
