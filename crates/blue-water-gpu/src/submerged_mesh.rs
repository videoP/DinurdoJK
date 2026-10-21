//! A sparse index set over Blue Water's own clipmap vertices for a view from
//! below the surface. The vertices, the per-level frames and the vertex
//! placement (`surface.wgsl`: snapping, the inner-edge shift, the outer-row
//! geomorph) are unchanged; only which lattice vertices the triangles use.
//!
//! From below, the surface only needs its lattice inside the refraction window
//! overhead. Beyond it the cells grow quickly with distance, and past the
//! distance where the water has absorbed everything each block is one quad.
//!
//! Each level is a quadtree of blocks aligned to their own size. A block's
//! perimeter keeps every vertex that is a corner of a neighbouring block, in the
//! same level or, across a level's outer row and the next level's hole, in the
//! other level, so no edge ends in a T-junction. Both seams are matched in
//! lattice indices: a level's outer row vertex `2j` and the next level's hole
//! vertex `j` are placed at the same point by the shader whatever the frames.
use crate::geometry::Clipmap;

/// Where `submerged_indices` keeps detail, all in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubmergedDetail {
    /// The point block distances are measured from (x, z).
    pub eye: [f32; 2],
    /// Inside this the original lattice is drawn: the refraction window overhead.
    pub near: f32,
    /// Past this the water is fully absorbed: blocks stay at the coarsest size.
    pub opaque: f32,
    /// Between `near` and `opaque` a block is about this share of its distance across.
    pub growth: f32,
}

#[derive(Clone, Copy)]
struct Block {
    x: i32,
    y: i32,
    size: i32,
}

/// Indices into `geometry::build`'s vertices for levels `first..=outer` (the
/// centre fill of `first`, then the rings). `frames` are the shader's
/// (`Clipmap::frames`). `admit` receives each candidate block's padded XZ
/// bounds in metres, as `OriginalClipmap::pruned_indices` does.
pub fn submerged_indices(
    clipmap: Clipmap,
    first: usize,
    outer: usize,
    frames: &[[f32; 4]; 10],
    detail: SubmergedDetail,
    mut admit: impl FnMut([f32; 2], [f32; 2]) -> bool,
) -> Vec<u32> {
    let segments = clipmap.segments as i32;
    let half = segments / 2;
    let hole = segments / 4;
    let largest = segments / 4;
    let side = (segments + 1) as usize;
    let per_level = side * side;
    let first = first.min(clipmap.levels - 1);
    let last = outer.max(first).min(clipmap.levels - 1);
    let slot = |x: i32, y: i32| (y + half) as usize * side + (x + half) as usize;
    // CPU copy of the shader's snapped placement (no geomorph: it moves a
    // vertex by at most one cell, which the admission pad covers).
    let position = |level: usize, x: i32, y: i32| -> [f32; 2] {
        let inner = level > 0 && x.abs().max(y.abs()) <= hole;
        let frame = if inner { frames[level - 1] } else { frames[level] };
        let cell = frames[level][2];
        [frame[0] + x as f32 * cell, frame[1] + y as f32 * cell]
    };
    let mut leaves: Vec<Vec<Block>> = vec![Vec::new(); last + 1];
    let mut active: Vec<Vec<bool>> = vec![Vec::new(); last + 1];
    for level in first..=last {
        let cell = frames[level][2];
        // The finest size a block at this distance may have: the original cells
        // in the window, then at least 4 cells (a 2-cell block is drawn as cells).
        let wanted = |distance: f32| -> i32 {
            if distance < detail.near {
                return 1;
            }
            if distance >= detail.opaque {
                return largest;
            }
            let across = (distance * detail.growth / cell).max(4.0);
            let mut size = 4;
            while size * 2 <= largest && (size * 2) as f32 <= across {
                size *= 2;
            }
            size.min(largest)
        };
        let mut stack: Vec<Block> = Vec::new();
        // The first level is its fill and its ring; every other level is a ring.
        for y in (-half..half).step_by(largest as usize) {
            for x in (-half..half).step_by(largest as usize) {
                let in_hole = level != first && x >= -hole && x < hole && y >= -hole && y < hole;
                if !in_hole {
                    stack.push(Block { x, y, size: largest });
                }
            }
        }
        let mut level_leaves = Vec::new();
        while let Some(block) = stack.pop() {
            let corners = [
                position(level, block.x, block.y),
                position(level, block.x + block.size, block.y),
                position(level, block.x, block.y + block.size),
                position(level, block.x + block.size, block.y + block.size),
            ];
            let mut lo = [f32::INFINITY; 2];
            let mut hi = [f32::NEG_INFINITY; 2];
            for corner in corners {
                for axis in 0..2 {
                    lo[axis] = lo[axis].min(corner[axis]);
                    hi[axis] = hi[axis].max(corner[axis]);
                }
            }
            let pad = cell * 1.05;
            if !admit([lo[0] - pad, lo[1] - pad], [hi[0] + pad, hi[1] + pad]) {
                continue;
            }
            let dx = (lo[0] - detail.eye[0]).max(detail.eye[0] - hi[0]).max(0.0);
            let dz = (lo[1] - detail.eye[1]).max(detail.eye[1] - hi[1]).max(0.0);
            let want = wanted(dx.hypot(dz));
            if block.size > want && block.size > 1 {
                let child = block.size / 2;
                for (ox, oy) in [(0, 0), (child, 0), (0, child), (child, child)] {
                    stack.push(Block { x: block.x + ox, y: block.y + oy, size: child });
                }
            } else {
                level_leaves.push(block);
            }
        }
        let mut marks = vec![false; per_level];
        for block in &level_leaves {
            for (x, y) in [(0, 0), (block.size, 0), (0, block.size), (block.size, block.size)] {
                marks[slot(block.x + x, block.y + y)] = true;
            }
        }
        leaves[level] = level_leaves;
        active[level] = marks;
    }
    // A level's outer row and the next level's hole edge share every vertex
    // either side uses. The outer row's odd vertices have no partner; the
    // shader's full outer-row geomorph folds them onto their even neighbours.
    for level in first..last {
        for i in (-half..=half).step_by(2) {
            let j = i / 2;
            for (outer_vertex, hole_vertex) in [
                ((i, -half), (j, -hole)),
                ((i, half), (j, hole)),
                ((-half, i), (-hole, j)),
                ((half, i), (hole, j)),
            ] {
                let a = slot(outer_vertex.0, outer_vertex.1);
                let b = slot(hole_vertex.0, hole_vertex.1);
                if active[level][a] || active[level + 1][b] {
                    active[level][a] = true;
                    active[level + 1][b] = true;
                }
            }
        }
    }
    let mut indices = Vec::new();
    for level in first..=last {
        let base = (level * per_level) as u32;
        let at = |x: i32, y: i32| base + slot(x, y) as u32;
        // `geometry::build`'s winding, by lattice position.
        let mut triangle = |a: (i32, i32), b: (i32, i32), c: (i32, i32)| {
            let cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            if cross > 0 {
                indices.extend([at(a.0, a.1), at(c.0, c.1), at(b.0, b.1)]);
            } else {
                indices.extend([at(a.0, a.1), at(b.0, b.1), at(c.0, c.1)]);
            }
        };
        let marks = &active[level];
        let mut perimeter: Vec<(i32, i32)> = Vec::new();
        for block in &leaves[level] {
            let (x0, y0, s) = (block.x, block.y, block.size);
            let (x1, y1) = (x0 + s, y0 + s);
            perimeter.clear();
            for y in y0..y1 {
                if y == y0 || marks[slot(x0, y)] { perimeter.push((x0, y)); }
            }
            for x in x0..x1 {
                if x == x0 || marks[slot(x, y1)] { perimeter.push((x, y1)); }
            }
            for y in (y0 + 1..=y1).rev() {
                if y == y1 || marks[slot(x1, y)] { perimeter.push((x1, y)); }
            }
            for x in (x0 + 1..=x1).rev() {
                if x == x1 || marks[slot(x, y0)] { perimeter.push((x, y0)); }
            }
            if perimeter.len() == 4 {
                // One diagonal direction everywhere, as `geometry::build`.
                triangle((x0, y0), (x0, y1), (x1, y1));
                triangle((x0, y0), (x1, y1), (x1, y0));
            } else {
                let apex = (x0 + s / 2, y0 + s / 2);
                for i in 0..perimeter.len() {
                    triangle(apex, perimeter[i], perimeter[(i + 1) % perimeter.len()]);
                }
            }
        }
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The shader's placement with its full outer-row geomorph, and no partial
    /// morph: a test eye at a frame centre keeps sparse blocks clear of it.
    fn placed(clipmap: Clipmap, frames: &[[f32; 4]; 10], vertex: u32) -> [i64; 2] {
        let segments = clipmap.segments as i32;
        let side = clipmap.segments + 1;
        let level = (vertex / (side * side)) as usize;
        let local = vertex % (side * side);
        let x = (local % side) as i32 - segments / 2;
        let y = (local / side) as i32 - segments / 2;
        let frame = frames[level];
        let inner = level > 0 && x.abs().max(y.abs()) <= segments / 4;
        let centre = if inner { frames[level - 1] } else { frame };
        let cell = frame[2];
        let mut index = [x as f32 + (centre[0] - frame[0]) / cell, y as f32 + (centre[1] - frame[1]) / cell];
        if x.abs().max(y.abs()) == segments / 2 {
            index = index.map(|v| v - v.rem_euclid(2.0));
        }
        // Exact in quarter-metre units for these power-of-two cells.
        [0, 1].map(|axis| ((frame[axis] + index[axis] * cell) * 4.0).round() as i64)
    }

    fn check_watertight(clipmap: Clipmap, frames: &[[f32; 4]; 10], first: usize, outer: usize, indices: &[u32]) {
        let mut edges: HashMap<([i64; 2], [i64; 2]), i32> = HashMap::new();
        let mut area = 0i64;
        for tri in indices.chunks_exact(3) {
            let p = [0, 1, 2].map(|i| placed(clipmap, frames, tri[i]));
            let twice = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            assert!(twice <= 0, "winding must match geometry::build: {p:?}");
            if twice == 0 { continue; }
            area -= twice;
            for i in 0..3 {
                let (a, b) = (p[i], p[(i + 1) % 3]);
                *edges.entry(if a < b { (a, b) } else { (b, a) }).or_default() += 1;
            }
        }
        let half = (clipmap.half(outer) * 4.0).round() as i64;
        let outer_frame = [frames[outer][0], frames[outer][1]].map(|v| (v * 4.0).round() as i64);
        assert_eq!(area, (2 * half) * (2 * half) * 2, "covers exactly the outer square");
        for ((a, b), count) in edges {
            let on_border = (0..2).any(|axis| {
                let lo = outer_frame[axis] - half;
                let hi = outer_frame[axis] + half;
                (a[axis] == lo && b[axis] == lo) || (a[axis] == hi && b[axis] == hi)
            });
            assert_eq!(count, if on_border { 1 } else { 2 }, "edge {a:?}-{b:?} (first {first})");
        }
    }

    fn detail(eye: [f32; 2]) -> SubmergedDetail {
        SubmergedDetail { eye, near: 10.0, opaque: 60.0, growth: 0.75 }
    }

    #[test]
    fn window_keeps_original_cells_and_far_water_is_sparse() {
        let clipmap = Clipmap::new(64, 6);
        let frames = clipmap.frames(0.0, 0.0);
        let indices = submerged_indices(clipmap, 0, 5, &frames, detail([0.0, 0.0]), |_, _| true);
        check_watertight(clipmap, &frames, 0, 5, &indices);
        let full = 64 * 64 * 2 + 5 * 64 * 64 * 3 / 2;
        assert!(indices.len() / 3 * 20 < full, "{} triangles of {full}", indices.len() / 3);
        // The window's 4 m cells survive: every unit cell within it is two triangles.
        let side = 65u32;
        let near_cells = indices.chunks_exact(3).filter(|tri| tri.iter().all(|&v| {
            let (x, y) = ((v % side) as i32 - 32, (v / side) as i32 - 32);
            v < side * side && x.abs() <= 2 && y.abs() <= 2
        })).count();
        assert!(near_cells >= 2 * 16, "{near_cells}");
    }

    #[test]
    fn shifted_frames_and_finer_levels_stay_watertight() {
        for (segments, levels, eye) in [(64, 6, [13.0, -29.0]), (128, 7, [-51.0, 7.0]), (64, 9, [3.0, 5.0])] {
            let clipmap = Clipmap::new(segments, levels);
            let frames = clipmap.frames(f64::from(eye[0]), f64::from(eye[1]));
            for first in [0, 1] {
                let indices = submerged_indices(clipmap, first, levels - 1, &frames,
                    detail(eye), |_, _| true);
                check_watertight(clipmap, &frames, first, levels - 1, &indices);
            }
        }
    }

    #[test]
    fn rejected_blocks_are_not_drawn() {
        let clipmap = Clipmap::new(64, 6);
        let frames = clipmap.frames(0.0, 0.0);
        let all = submerged_indices(clipmap, 0, 5, &frames, detail([0.0, 0.0]), |_, _| true);
        let east = submerged_indices(clipmap, 0, 5, &frames, detail([0.0, 0.0]), |_, hi| hi[0] > 0.0);
        assert!(east.len() < all.len());
        assert!(submerged_indices(clipmap, 0, 5, &frames, detail([0.0, 0.0]), |_, _| false).is_empty());
    }
}
