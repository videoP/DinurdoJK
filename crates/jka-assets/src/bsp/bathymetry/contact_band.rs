//! Static contact field for the renderer's Gerstner ocean (`r_oceanType 5`): godot-water-shader's intersection foam
//! reads two orthographic depth cameras, 10 m above and below the water, for the highest and lowest surface over each
//! column. A JKA map's static solids give the same answer without rendering them.
use super::Geometry;

impl Geometry {
    /// Z-up/JKA XY columns at `origin + (x, y) * cell`: the highest and lowest static solid within `band` of `level`,
    /// relative to `level`, in map units. Brush volumes count clipped to the band (upstream's cameras clip at their near
    /// and far planes); planar, patch and triangle surfaces count where they cross it. A column with neither is
    /// `[-band, band]`, the cameras' far planes, which never reads as contact.
    pub fn contact_band(
        &self,
        origin: [f32; 2],
        size: [usize; 2],
        cell: f32,
        level: f32,
        band: f32,
    ) -> Result<Vec<[f32; 2]>, String> {
        let [nx, ny] = size;
        if nx < 1
            || ny < 1
            || nx > 4096
            || ny > 4096
            || !cell.is_finite()
            || cell < 1.0
            || !level.is_finite()
            || !band.is_finite()
            || band <= 0.0
            || origin.iter().any(|v| !v.is_finite())
        {
            return Err("invalid contact band raster bounds".into());
        }
        let low_z = f64::from(level - band);
        let high_z = f64::from(level + band);
        let mut spans = vec![[-band, band]; nx * ny];
        // Bin the static geometry by small raster blocks so distant brushes never enter a column solve.
        for y0 in (0..ny).step_by(16) {
            for x0 in (0..nx).step_by(16) {
                let x1 = (x0 + 16).min(nx);
                let y1 = (y0 + 16).min(ny);
                let low = [origin[0] + x0 as f32 * cell, origin[1] + y0 as f32 * cell];
                let high = [
                    origin[0] + (x1 - 1) as f32 * cell,
                    origin[1] + (y1 - 1) as f32 * cell,
                ];
                let overlaps =
                    |b: [[f32; 2]; 2]| (0..2).all(|a| b[1][a] >= low[a] && b[0][a] <= high[a]);
                let solids: Vec<_> = self.solids.iter().filter(|s| overlaps(s.bounds)).collect();
                let triangles: Vec<_> = self.triangles.iter().filter(|t| overlaps(t.bounds)).collect();
                if solids.is_empty() && triangles.is_empty() {
                    continue;
                }
                for y in y0..y1 {
                    for x in x0..x1 {
                        let at = [origin[0] + x as f32 * cell, origin[1] + y as f32 * cell];
                        let (mut top, mut bottom) = (f64::NEG_INFINITY, f64::INFINITY);
                        for solid in &solids {
                            let Some((from, to, _)) = solid.column(at) else {
                                continue;
                            };
                            if to < low_z || from > high_z {
                                continue;
                            }
                            top = top.max(to.min(high_z));
                            bottom = bottom.min(from.max(low_z));
                        }
                        for triangle in &triangles {
                            let Some(z) = triangle.height(at) else {
                                continue;
                            };
                            if (low_z..=high_z).contains(&z) {
                                top = top.max(z);
                                bottom = bottom.min(z);
                            }
                        }
                        if top.is_finite() && bottom.is_finite() {
                            spans[y * nx + x] = [
                                (top - f64::from(level)) as f32,
                                (bottom - f64::from(level)) as f32,
                            ];
                        }
                    }
                }
            }
        }
        Ok(spans)
    }
}
