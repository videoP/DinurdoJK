//! `r_oceanType 5`'s intersection-foam source. godot-water-shader renders two orthographic depth cameras 10 m above
//! and below the water for the highest and lowest surface over each column (`shaders/i_depth.gdshader`); a JKA map's
//! static solids give that answer once per map (`Geometry::contact_band`), built on a worker the first time the mode
//! asks for it.
use super::OCEAN_UNITS_PER_METER;
use std::sync::Arc;

/// Upstream's depth cameras stand 10 m above and below the water with a 20 m far plane.
const BAND_METRES: f32 = 10.0;
/// Margin around a pool's outline: its waves' horizontal reach and the foam's bilinear footprint (m).
const HALO_METRES: f32 = 4.0;
/// Finest texel (map units, 0.25 m; upstream's 128-texel, 30 m capture is 0.23 m) and the largest edge.
const MIN_CELL_UNITS: f32 = 10.0;
const MAX_TEXELS: f32 = 1024.0;

/// One pool's field, in the ocean's grid metres (x, z), rows along +z.
pub struct ContactField {
    /// The bathymetry footprint's pool slot (diagnostics; the renderer matches fields by plane and extent).
    pub slot: usize,
    /// The pool's plane (map units, renderer y).
    pub level: f32,
    pub size: [u32; 2],
    /// The first texel's centre (grid m).
    pub origin: [f32; 2],
    pub cell: f32,
    /// [highest, lowest] solid within the band, metres relative to `level`.
    pub texels: Vec<[f32; 2]>,
}

pub type ContactFields = Arc<Vec<ContactField>>;

/// `footprints`: (pool slot, plane height in map units, outline triangles in BSP XY).
pub(super) fn build(
    bsp: &jka_assets::bsp::Bsp,
    footprints: &[(u32, f32, Vec<jka_assets::bsp::bathymetry::FootprintTriangle>)],
) -> Result<Vec<ContactField>, String> {
    let geometry = jka_assets::bsp::bathymetry::Geometry::from_bsp(bsp)?;
    let mut fields = Vec::new();
    for (slot, level, triangles) in footprints {
        if triangles.is_empty() {
            continue;
        }
        let mut low = [f32::INFINITY; 2];
        let mut high = [f32::NEG_INFINITY; 2];
        for point in triangles.iter().flatten() {
            for axis in 0..2 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
        let halo = HALO_METRES * OCEAN_UNITS_PER_METER;
        for axis in 0..2 {
            low[axis] -= halo;
            high[axis] += halo;
        }
        let cell = ((high[0] - low[0]).max(high[1] - low[1]) / MAX_TEXELS).max(MIN_CELL_UNITS);
        let size = [0, 1].map(|axis| ((high[axis] - low[axis]) / cell).ceil() as usize + 1);
        let band = BAND_METRES * OCEAN_UNITS_PER_METER;
        let spans = geometry.contact_band(low, size, cell, *level, band)?;
        // BSP +Y is the renderer's -Z: flip the rows so they run along grid +z.
        let mut texels = Vec::with_capacity(spans.len());
        for row in spans.chunks_exact(size[0]).rev() {
            texels.extend(row.iter().map(|span| span.map(|v| v / OCEAN_UNITS_PER_METER)));
        }
        let touching = texels.iter().filter(|span| span[0] > -BAND_METRES).count();
        eprintln!(
            "GERSTNER CONTACT: pool {} field {}x{}, {:.2} m/texel, {touching} texels with solids near the water",
            slot + 1, size[0], size[1], cell / OCEAN_UNITS_PER_METER
        );
        fields.push(ContactField {
            slot: *slot as usize,
            level: *level,
            size: size.map(|v| v as u32),
            origin: [low[0] / OCEAN_UNITS_PER_METER, -(low[1] + (size[1] - 1) as f32 * cell) / OCEAN_UNITS_PER_METER],
            cell: cell / OCEAN_UNITS_PER_METER,
            texels,
        });
    }
    Ok(fields)
}
