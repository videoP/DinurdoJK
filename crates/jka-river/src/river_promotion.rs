//! Prepared, bounded authored river domain. All coordinates are BSP map units.
pub const MAX_PLANS: usize = 4;
pub const MAX_CELLS: usize = 180_000;
pub const ID_BIT: u64 = 1 << 63;

pub fn is_promoted(id: u64) -> bool {
    id & ID_BIT != 0
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Cell {
    pub level: f32,
    pub bed: f32,
    pub velocity: [f32; 2],
    pub wet: bool,
    /// Signed boundary exchange rate in units/s (positive inlet, negative outlet).
    pub exchange: f32,
    pub target: f32,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub id: u64,
    pub corner: [f32; 2],
    pub dx: f32,
    pub dims: [u32; 2],
    pub cells: Vec<Cell>,
    /// (BSP surface, inline model, BSP shader).
    pub surfaces: Vec<(usize, u32, usize)>,
    pub replace: bool,
    /// Sorted exact authored world vertex ranges, including all sibling stages.
    pub world_ranges: Vec<(u32, u32)>,
}
