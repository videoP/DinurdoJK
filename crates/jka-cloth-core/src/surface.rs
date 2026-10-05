use crate::Vec3;

/// One-to-one simulation/render vertex mapping in 0.1. Indices are stable for
/// the cloth lifetime. The borrow prevents stepping while these slices are live.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceView<'a> {
    pub positions: &'a [Vec3],
    pub triangles: &'a [[u32; 3]],
}
impl SurfaceView<'_> {
    /// Area-weighted normals. Returns the number of degenerate faces; zero
    /// normals on isolated/fully degenerate vertices are retained as diagnostics.
    pub fn write_normals(&self, out: &mut Vec<Vec3>) -> usize {
        out.resize(self.positions.len(), Vec3::ZERO);
        out.fill(Vec3::ZERO);
        let mut degenerate = 0;
        for &[a, b, c] in self.triangles {
            let n = (self.positions[b as usize] - self.positions[a as usize])
                .cross(self.positions[c as usize] - self.positions[a as usize]);
            if !n.is_finite() || n.length_squared() <= 0.0 {
                degenerate += 1;
                continue;
            }
            for i in [a, b, c] {
                out[i as usize] += n;
            }
        }
        for n in out {
            *n = n.normalize_or_zero();
        }
        degenerate
    }
}
