//! Sparse tile residency for long-distance rivers.
//!
//! Not an Unreal system. UE 5.8.1 Grid2D_SW_River simulates ONE dense
//! rectangle over the combined source/sink water-body bounds
//! (ShallowWaterRiverActor.cpp Rebuild), and Niagara's Grid2DCollection has no
//! sparse or growable storage (SetNumCells reallocates and resets). This module
//! only changes WHERE cells live. The domain stays a virtual dense rectangle
//! (`Lattice`) with dense coordinates, dimensions and edge policy, so every
//! stage keeps the dense per-cell arithmetic; only square tiles of it are
//! resident in a slot pool.
//!
//! Reads inside the lattice that hit a non-resident tile return dry, still
//! water over a collider: the native two-cell border wall. Water reaching an
//! unexpanded edge is therefore reflected, never deleted, and a later tile
//! activation cannot create water. Reads outside the lattice keep the dense
//! zero policy. A resident tile that has stayed dry is indistinguishable from
//! a dense grid's dry cells (height = bottom, zero depth and velocity, history
//! = bottom), which is what `TileInit` writes.

use super::{Geometry, Water};

pub const MIN_TILE: u32 = 8;
pub const MAX_TILE: u32 = 256;
/// Upper bound on table entries (a 4096x4096-tile lattice).
pub const MAX_TABLE: u64 = 1 << 24;

/// Geometry returned for in-lattice, non-resident cells: the collider wall.
pub const UNRESIDENT_GEOMETRY: Geometry = Geometry {
    bottom: 0.0,
    collider: 1.0,
    collision_velocity: [0.0; 2],
};

/// Virtual dense rectangle partitioned into square power-of-two tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lattice {
    tile: u32,
    tiles: [u32; 2],
}

impl Lattice {
    pub fn new(tile: u32, tiles: [u32; 2]) -> Result<Self, &'static str> {
        if !tile.is_power_of_two() || !(MIN_TILE..=MAX_TILE).contains(&tile) {
            return Err("river tile size must be a power of two in 8..=256");
        }
        if tiles.contains(&0)
            || u64::from(tiles[0]) * u64::from(tiles[1]) > MAX_TABLE
            || tiles
                .iter()
                .any(|t| u64::from(*t) * u64::from(tile) > i32::MAX as u64 / 2)
        {
            return Err("river tile lattice is empty or exceeds the table limit");
        }
        Ok(Self { tile, tiles })
    }

    /// Smallest lattice of `tile`-sized tiles covering `cells`.
    pub fn covering(tile: u32, cells: [u32; 2]) -> Result<Self, &'static str> {
        if !tile.is_power_of_two() || tile == 0 {
            return Err("river tile size must be a power of two in 8..=256");
        }
        Self::new(tile, cells.map(|n| n.div_ceil(tile)))
    }

    pub fn tile(self) -> u32 {
        self.tile
    }
    pub fn shift(self) -> u32 {
        self.tile.trailing_zeros()
    }
    pub fn tiles(self) -> [u32; 2] {
        self.tiles
    }
    pub fn tile_cells(self) -> usize {
        (self.tile * self.tile) as usize
    }
    /// Virtual dense dimensions seen by every stage.
    pub fn dimensions(self) -> [u32; 2] {
        self.tiles.map(|t| t * self.tile)
    }
    pub fn contains(self, at: [i32; 2]) -> bool {
        let d = self.dimensions();
        at[0] >= 0 && at[1] >= 0 && (at[0] as u32) < d[0] && (at[1] as u32) < d[1]
    }
    pub fn contains_tile(self, tile: [i32; 2]) -> bool {
        tile[0] >= 0
            && tile[1] >= 0
            && (tile[0] as u32) < self.tiles[0]
            && (tile[1] as u32) < self.tiles[1]
    }
    /// Tile holding an in-lattice cell.
    pub fn tile_of(self, at: [i32; 2]) -> [i32; 2] {
        at.map(|v| v >> self.shift())
    }
    pub fn tile_origin(self, tile: [i32; 2]) -> [i32; 2] {
        tile.map(|v| v << self.shift())
    }
}

/// Dense tile -> slot table over the lattice. Slots are appended in activation
/// order; storage index = slot * tile_cells + local_y * tile + local_x.
#[derive(Clone, Debug)]
pub struct Residency {
    lattice: Lattice,
    table: Vec<i32>,
    slots: Vec<[i32; 2]>,
}

impl Residency {
    pub fn new(lattice: Lattice) -> Self {
        let [w, h] = lattice.tiles();
        Self {
            lattice,
            table: vec![-1; w as usize * h as usize],
            slots: Vec::new(),
        }
    }

    /// Every tile resident in row-major order (the dense-equivalence layout).
    pub fn full(lattice: Lattice) -> Self {
        let mut residency = Self::new(lattice);
        let [w, h] = lattice.tiles();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                residency.insert([x, y]).expect("tile is inside the lattice");
            }
        }
        residency
    }

    pub fn lattice(&self) -> Lattice {
        self.lattice
    }

    fn table_index(&self, tile: [i32; 2]) -> Option<usize> {
        self.lattice
            .contains_tile(tile)
            .then(|| tile[1] as usize * self.lattice.tiles()[0] as usize + tile[0] as usize)
    }

    pub fn slot(&self, tile: [i32; 2]) -> Option<u32> {
        let slot = *self.table.get(self.table_index(tile)?)?;
        (slot >= 0).then_some(slot as u32)
    }

    pub fn is_resident(&self, tile: [i32; 2]) -> bool {
        self.slot(tile).is_some()
    }

    /// Make a tile resident. Returns its slot and whether it was new.
    pub fn insert(&mut self, tile: [i32; 2]) -> Result<(u32, bool), &'static str> {
        let i = self
            .table_index(tile)
            .ok_or("river tile is outside the lattice")?;
        if self.table[i] >= 0 {
            return Ok((self.table[i] as u32, false));
        }
        let slot = u32::try_from(self.slots.len()).map_err(|_| "river tile slot overflow")?;
        if slot >= i32::MAX as u32 {
            return Err("river tile slot overflow");
        }
        self.table[i] = slot as i32;
        self.slots.push(tile);
        Ok((slot, true))
    }

    /// Make a tile non-resident (sparse tile eviction; not Unreal). The last
    /// slot moves into the freed one so slots stay dense: returns the freed
    /// slot and, when another tile moved, (its old slot, the tile).
    pub fn swap_remove(&mut self, tile: [i32; 2]) -> Option<(u32, Option<(u32, [i32; 2])>)> {
        let i = self.table_index(tile)?;
        let slot = u32::try_from(self.table[i]).ok()?;
        self.table[i] = -1;
        let last = self.slots.len() as u32 - 1;
        self.slots.swap_remove(slot as usize);
        if slot == last {
            return Some((slot, None));
        }
        let moved = self.slots[slot as usize];
        let j = self.table_index(moved).expect("resident tile is inside the lattice");
        self.table[j] = slot as i32;
        Some((slot, Some((last, moved))))
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }
    pub fn slot_tile(&self, slot: u32) -> [i32; 2] {
        self.slots[slot as usize]
    }
    pub fn slot_tiles(&self) -> &[[i32; 2]] {
        &self.slots
    }
    /// GPU tile -> slot table, row-major over the lattice, -1 when absent.
    pub fn table(&self) -> &[i32] {
        &self.table
    }
    pub fn cell_count(&self) -> usize {
        self.slots.len() * self.lattice.tile_cells()
    }

    /// Storage index of an in-lattice resident cell.
    pub fn index(&self, at: [i32; 2]) -> Option<usize> {
        if !self.lattice.contains(at) {
            return None;
        }
        let slot = self.slot(self.lattice.tile_of(at))? as usize;
        let mask = self.lattice.tile() as i32 - 1;
        let tile = self.lattice.tile() as usize;
        Some(slot * self.lattice.tile_cells() + (at[1] & mask) as usize * tile + (at[0] & mask) as usize)
    }

    /// Lattice cell of a storage index.
    pub fn cell(&self, index: usize) -> [i32; 2] {
        let cells = self.lattice.tile_cells();
        let tile = self.lattice.tile() as usize;
        let origin = self.lattice.tile_origin(self.slots[index / cells]);
        let local = index % cells;
        [origin[0] + (local % tile) as i32, origin[1] + (local / tile) as i32]
    }

    /// Tiles within `ring` tiles (Chebyshev) of `tile`, inside the lattice.
    pub fn neighbourhood(&self, tile: [i32; 2], ring: u32) -> impl Iterator<Item = [i32; 2]> + '_ {
        let r = ring as i32;
        (-r..=r)
            .flat_map(move |dy| (-r..=r).map(move |dx| [tile[0] + dx, tile[1] + dy]))
            .filter(|t| self.lattice.contains_tile(*t))
    }
}

/// State of a newly activated tile: what a dense grid's cell holds after it
/// has stayed dry for at least one completed frame.
pub struct TileInit;

impl TileInit {
    pub fn water(geometry: Geometry) -> Water {
        Water {
            height: geometry.bottom,
            ..Water::default()
        }
    }
    /// PreSim history after a completed frame equals the current bottom. The
    /// reset zero applies only to a simulation's first frame; using it for a
    /// later tile would make advection delete water entering the tile.
    pub fn previous_bottom(geometry: Geometry) -> f32 {
        geometry.bottom
    }
}

/// Worst-case number of cells the wet front can advance per substep. Depth
/// changes only through face fluxes with direct neighbours (update_depth) and
/// advection, which only runs on already-wet cells; velocity extrapolation
/// into dry cells is disabled in the cached river variant.
pub const WET_FRONT_CELLS_PER_SUBSTEP: u32 = 1;

/// Largest stencil distance from a wet cell at which a non-resident cell
/// would change a result (update_velocity/compute_normals neighbours, plus
/// one for the depth stencil's second neighbour).
pub const WET_STENCIL_MARGIN: u32 = 3;

/// How many solver frames may run before fresh occupancy is required, given
/// `margin_cells` between the nearest occupied cell and a non-resident cell.
pub fn solver_frames_within_margin(margin_cells: u32, substeps: u32) -> u32 {
    let per_frame = substeps.max(1) * WET_FRONT_CELLS_PER_SUBSTEP;
    margin_cells.saturating_sub(WET_STENCIL_MARGIN) / per_frame
}

/// Tiles that must be resident for a set of occupied tiles: every tile within
/// `guard` tiles of an occupied one. The occupied tiles are included.
pub fn required_tiles(
    lattice: Lattice,
    occupied: impl IntoIterator<Item = [i32; 2]>,
    guard: u32,
) -> Vec<[i32; 2]> {
    let mut seen = std::collections::BTreeSet::new();
    let r = guard as i32;
    for t in occupied {
        for dy in -r..=r {
            for dx in -r..=r {
                let n = [t[0] + dx, t[1] + dy];
                if lattice.contains_tile(n) {
                    seen.insert([n[1], n[0]]);
                }
            }
        }
    }
    seen.into_iter().map(|[y, x]| [x, y]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_and_residency_round_trip_cells() {
        let lattice = Lattice::new(8, [3, 2]).unwrap();
        assert_eq!(lattice.dimensions(), [24, 16]);
        assert!(Lattice::new(12, [1, 1]).is_err());
        assert!(Lattice::new(8, [0, 1]).is_err());
        assert_eq!(Lattice::covering(8, [17, 8]).unwrap().tiles(), [3, 1]);
        let mut r = Residency::new(lattice);
        assert_eq!(r.insert([2, 1]).unwrap(), (0, true));
        assert_eq!(r.insert([0, 0]).unwrap(), (1, true));
        assert_eq!(r.insert([2, 1]).unwrap(), (0, false));
        assert!(r.insert([3, 0]).is_err());
        assert_eq!(r.index([17, 9]), Some(9));
        assert_eq!(r.index([5, 5]), Some(64 + 45));
        assert_eq!(r.index([9, 0]), None);
        assert_eq!(r.index([-1, 0]), None);
        for i in 0..r.cell_count() {
            assert_eq!(r.index(r.cell(i)), Some(i));
        }
        assert_eq!(r.table(), &[1, -1, -1, -1, -1, 0]);
    }

    #[test]
    fn swap_remove_keeps_slots_dense() {
        let lattice = Lattice::new(8, [4, 4]).unwrap();
        let mut r = Residency::new(lattice);
        for t in [[0, 0], [1, 0], [2, 0], [3, 0]] {
            r.insert(t).unwrap();
        }
        assert_eq!(r.swap_remove([1, 0]), Some((1, Some((3, [3, 0])))));
        assert_eq!(r.slot([3, 0]), Some(1));
        assert_eq!(r.slot([1, 0]), None);
        assert_eq!(r.swap_remove([2, 0]), Some((2, None)));
        assert_eq!(r.slot_tiles(), &[[0, 0], [3, 0]]);
        assert_eq!(r.swap_remove([2, 0]), None);
        assert_eq!(r.insert([1, 0]).unwrap(), (2, true));
    }

    #[test]
    fn guard_ring_and_margin_budget() {
        let lattice = Lattice::new(32, [4, 4]).unwrap();
        let tiles = required_tiles(lattice, [[0, 0]], 1);
        assert_eq!(tiles, vec![[0, 0], [1, 0], [0, 1], [1, 1]]);
        assert_eq!(required_tiles(lattice, [[2, 2], [2, 2]], 1).len(), 9);
        // One 32-cell guard tile, 10 substeps: two solver frames.
        assert_eq!(solver_frames_within_margin(32, 10), 2);
        assert_eq!(solver_frames_within_margin(2, 10), 0);
    }
}
