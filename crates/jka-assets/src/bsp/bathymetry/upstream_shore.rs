//! Static height field for the separate Blue Water shore renderer; never changes the shallow-water solver atlas.
use super::Geometry;

impl Geometry {
    /// Z-up/JKA XY samples at `origin + (x,y)*cell`, relative to `level`, in map units.
    /// Unknown or ambiguous columns remain NaN. The renderer leaves their waves unmodified.
    pub fn upstream_shore_heights(
        &self,
        origin: [f32; 2],
        size: [usize; 2],
        cell: f32,
        level: f32,
        footprint: &[super::FootprintTriangle],
    ) -> Result<Vec<f32>, String> {
        let [nx, ny] = size;
        if nx < 2
            || ny < 2
            || nx > 1025
            || ny > 1025
            || !cell.is_finite()
            || cell < 1.0
            || !level.is_finite()
            || origin.iter().any(|v| !v.is_finite())
        {
            return Err("invalid upstream shore raster bounds".into());
        }
        let mut heights = vec![f32::NAN; nx * ny];
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
                let solids: Vec<_> = self
                    .solids
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| overlaps(s.bounds).then_some(i))
                    .collect();
                let triangles: Vec<_> = self
                    .triangles
                    .iter()
                    .enumerate()
                    .filter_map(|(i, t)| overlaps(t.bounds).then_some(i))
                    .collect();
                let water: Vec<_> = footprint
                    .iter()
                    .copied()
                    .filter(|t| overlaps(super::bounds(t.iter().copied())))
                    .collect();
                for y in y0..y1 {
                    for x in x0..x1 {
                        let at = [origin[0] + x as f32 * cell, origin[1] + y as f32 * cell];
                        let column = self.column_with_sky(at, level, &solids, &triangles, true);
                        if !column.overhang {
                            let mut bed = column.bed;
                            if !water.iter().any(|t| super::contains(*t, at)) {
                                // Triangle terrain can rise above water without a solid brush. Outside the
                                // authored water, read its nearest upward bank too; above wet water, retain
                                // the column solve so bridge decks never become fictitious islands.
                                let bank = triangles
                                    .iter()
                                    .filter_map(|&i| {
                                        let t = &self.triangles[i];
                                        (t.normal[2] > super::EPS).then(|| t.height(at)).flatten()
                                    })
                                    .filter(|h| *h >= f64::from(level))
                                    .min_by(f64::total_cmp);
                                if let Some(bank) = bank {
                                    bed = Some(bed.map_or(bank as f32, |b| b.max(bank as f32)));
                                }
                            }
                            if let Some(bed) = bed {
                                heights[y * nx + x] = bed - level;
                            }
                        }
                    }
                }
            }
        }
        Ok(heights)
    }
}
