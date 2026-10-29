//! TaystJK/Id/Raven curved-grid LOD. Reference: TaystJK b35ed06f,
//! codemp/rd-vanilla/{tr_curve,tr_bsp,tr_surface}.cpp (GPL-2.0).
//! Visual meshes are deliberately separate from collision/acoustic meshes.
use super::*;

#[derive(Debug, Clone)]
pub struct MeshPatch {
    pub surface: usize,
    pub vertices: Range<usize>,
    pub lod: PatchLod,
}

#[derive(Debug, Clone)]
pub struct PatchLod {
    pub width: usize,
    pub height: usize,
    pub width_lod_error: Vec<f32>,
    pub height_lod_error: Vec<f32>,
    pub lod_origin: [f32; 3],
    pub lod_radius: f32,
    pub subdivision_error: f32,
    pub stitched_vertices: usize,
}

impl PatchLod {
    pub(super) fn from_grid(grid: &PatchGrid, bounds: [[f32; 3]; 2]) -> Self {
        let lod_origin = std::array::from_fn(|i| (bounds[0][i] + bounds[1][i]) * 0.5);
        let delta = std::array::from_fn(|i| bounds[0][i] - lod_origin[i]);
        Self {
            width: grid.width,
            height: grid.height,
            width_lod_error: grid.width_lod_error.clone(),
            height_lod_error: grid.height_lod_error.clone(),
            lod_origin,
            lod_radius: dot3(delta, delta).sqrt(),
            subdivision_error: grid.subdivision_error,
            stitched_vertices: grid.stitched_vertices,
        }
    }

    pub fn full_triangles(&self) -> usize {
        (self.width - 1) * (self.height - 1) * 2
    }

    /// Origin/forward must use the same coordinate space as lod_origin. For an
    /// inline model, transform the view into the model's local space first.
    pub fn threshold(
        &self,
        view_origin: [f32; 3],
        view_forward: [f32; 3],
        curve_error: f32,
    ) -> f32 {
        if curve_error < 0.0 {
            return 0.0;
        }
        let delta = std::array::from_fn(|i| self.lod_origin[i] - view_origin[i]);
        curve_error / (dot3(delta, view_forward).abs() - self.lod_radius).max(1.0)
    }

    /// 65-bit masks include both boundaries and use the original <= comparison.
    /// Comparing masks avoids allocating/rebuilding indices for unchanged LOD.
    pub fn selection(&self, threshold: f32) -> [u128; 2] {
        fn axis(errors: &[f32], threshold: f32) -> u128 {
            let mut mask = 1 | (1 << (errors.len() - 1));
            for (i, &error) in errors.iter().enumerate().take(errors.len() - 1).skip(1) {
                if error <= threshold {
                    mask |= 1 << i;
                }
            }
            mask
        }
        [
            axis(&self.width_lod_error, threshold),
            axis(&self.height_lod_error, threshold),
        ]
    }

    /// Winding after the client's JKA -> renderer axis conversion. Every index
    /// refers to an original load-time grid vertex; no attributes are regenerated.
    pub fn append_indices(&self, masks: [u128; 2], grid_vertices: &[u32], out: &mut Vec<u32>) {
        let mut rows = masks[1];
        let mut y = rows.trailing_zeros() as usize;
        rows &= rows - 1;
        while rows != 0 {
            let next_y = rows.trailing_zeros() as usize;
            let mut cols = masks[0];
            let mut x = cols.trailing_zeros() as usize;
            cols &= cols - 1;
            while cols != 0 {
                let next_x = cols.trailing_zeros() as usize;
                let a = grid_vertices[y * self.width + x];
                let b = grid_vertices[y * self.width + next_x];
                let c = grid_vertices[next_y * self.width + x];
                let d = grid_vertices[next_y * self.width + next_x];
                out.extend_from_slice(&[a, b, c, b, d, c]);
                x = next_x;
                cols &= cols - 1;
            }
            y = next_y;
            rows &= rows - 1;
        }
    }
}

impl Bsp {
    /// World and inline visual meshes from one subdivision/stitch pass. Stitching
    /// covers every patch in the BSP, so building the two meshes separately
    /// would run it twice.
    pub fn visual_meshes(
        &self,
        subdivisions: f32,
        stitching: bool,
    ) -> Result<(Mesh, (Mesh, Vec<u32>))> {
        let grids = self.visual_patch_grids(subdivisions, stitching)?;
        Ok((self.world_mesh_from_grids(&grids)?, self.inline_mesh_from_grids(&grids)?))
    }

    pub fn world_visual_mesh(&self, subdivisions: f32, stitching: bool) -> Result<Mesh> {
        let grids = self.visual_patch_grids(subdivisions, stitching)?;
        self.world_mesh_from_grids(&grids)
    }

    pub fn inline_visual_mesh(
        &self,
        subdivisions: f32,
        stitching: bool,
    ) -> Result<(Mesh, Vec<u32>)> {
        let grids = self.visual_patch_grids(subdivisions, stitching)?;
        self.inline_mesh_from_grids(&grids)
    }

    fn world_mesh_from_grids(&self, grids: &[Option<PatchGrid>]) -> Result<Mesh> {
        let mut mesh = Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            batches: Vec::new(),
            patches: Vec::new(),
        };
        self.append_model_mesh(&mut mesh, 0, 4, Some(grids))?;
        Ok(mesh)
    }

    fn inline_mesh_from_grids(&self, grids: &[Option<PatchGrid>]) -> Result<(Mesh, Vec<u32>)> {
        let mut mesh = Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            batches: Vec::new(),
            patches: Vec::new(),
        };
        let mut models = Vec::new();
        for model in 1..self.models.len() {
            self.append_model_mesh(&mut mesh, model, 4, Some(grids))?;
            models.resize(mesh.batches.len(), model as u32);
        }
        Ok((mesh, models))
    }

    fn visual_patch_grids(
        &self,
        subdivisions: f32,
        stitching: bool,
    ) -> Result<Vec<Option<PatchGrid>>> {
        if !subdivisions.is_finite() || !(0.0..=80.0).contains(&subdivisions) {
            return Err(invalid("r_subdivisions must be finite and in 0..80"));
        }
        let mut patch_vertices = 0usize;
        let mut patch_indices = 0usize;
        let mut grids = self
            .surfaces
            .iter()
            .map(|s| {
                if s.kind != SurfaceKind::Patch
                    || self.shaders[s.shader].surface_flags & SURF_NODRAW != 0
                {
                    return Ok(None);
                }
                let mut grid = subdivide_patch_to_grid(
                    s.patch_size[0],
                    s.patch_size[1],
                    &self.vertices[s.vertices.clone()],
                    subdivisions,
                )?;
                orient_grid(&mut grid);
                patch_vertices += grid.vertices.len();
                patch_indices += (grid.width - 1) * (grid.height - 1) * 6;
                if patch_vertices > MAX_MESH_VERTICES * 2 || patch_indices > MAX_MESH_INDICES * 2 {
                    return Err(invalid(
                        "visual patch grids exceed the world/inline mesh memory budget",
                    ));
                }
                Ok(Some(grid))
            })
            .collect::<Result<Vec<_>>>()?;
        // Group by exact authored origin/radius, preserving BSP order. This
        // removes unrelated O(N^2) comparisons without changing traversal.
        let mut groups = std::collections::BTreeMap::<[u32; 4], Vec<usize>>::new();
        for (i, grid) in grids.iter().enumerate() {
            if let Some(grid) = grid {
                let lod = PatchLod::from_grid(grid, self.surfaces[i].patch_lod_bounds);
                let key = [
                    lod.lod_origin[0],
                    lod.lod_origin[1],
                    lod.lod_origin[2],
                    lod.lod_radius,
                ]
                .map(|f| if f == 0.0 { 0 } else { f.to_bits() });
                groups.entry(key).or_default().push(i);
            }
        }
        for group in groups.values() {
            if stitching {
                stitch_all(&mut grids, group);
            }
            fix_shared_errors(&mut grids, group);
        }
        Ok(grids)
    }
}

fn orient_grid(grid: &mut PatchGrid) {
    if grid.height > grid.width {
        let mut rows: Vec<_> = grid
            .vertices
            .chunks_exact(grid.width)
            .map(|r| r.to_vec())
            .collect();
        rows = transpose_patch_grid(&rows, grid.width, grid.height);
        for row in &mut rows {
            row.reverse();
        }
        std::mem::swap(&mut grid.width, &mut grid.height);
        std::mem::swap(&mut grid.width_lod_error, &mut grid.height_lod_error);
        grid.width_lod_error.reverse();
        make_patch_normals(&mut rows, grid.width, grid.height);
        grid.vertices = rows.into_iter().flatten().collect();
    }
}

// Edges are ordered width0,width1,height0,height1, as in tr_bsp.cpp.
fn edge_len(g: &PatchGrid, e: usize) -> usize {
    if e < 2 {
        g.width
    } else {
        g.height
    }
}
fn edge_index(g: &PatchGrid, e: usize, p: usize) -> usize {
    match e {
        0 => p,
        1 => (g.height - 1) * g.width + p,
        2 => p * g.width,
        _ => p * g.width + g.width - 1,
    }
}
fn edge_error(g: &PatchGrid, e: usize, p: usize) -> f32 {
    if e < 2 {
        g.width_lod_error[p]
    } else {
        g.height_lod_error[p]
    }
}
fn close(a: [f32; 3], b: [f32; 3], tolerance: f64) -> bool {
    (0..3).all(|i| f64::from((a[i] - b[i]).abs()) <= tolerance)
}
fn merged(g: &PatchGrid, e: usize) -> bool {
    for i in 1..edge_len(g, e) - 1 {
        for j in i + 1..edge_len(g, e) - 1 {
            if close(
                g.vertices[edge_index(g, e, i)].position,
                g.vertices[edge_index(g, e, j)].position,
                0.1,
            ) {
                return true;
            }
        }
    }
    false
}

fn insert(g: &mut PatchGrid, edge: usize, at: usize, point: [f32; 3], error: f32) {
    let mut rows: Vec<_> = g
        .vertices
        .chunks_exact(g.width)
        .map(|r| r.to_vec())
        .collect();
    if edge < 2 {
        let boundary = if edge == 0 { 0 } else { g.height - 1 };
        for (y, row) in rows.iter_mut().enumerate() {
            let mut v = lerp_draw_vertex(row[at - 1], row[at]);
            if y == boundary {
                v.position = point;
            }
            row.insert(at, v);
        }
        g.width += 1;
        g.width_lod_error.insert(at, error);
    } else {
        let boundary = if edge == 2 { 0 } else { g.width - 1 };
        let row = (0..g.width)
            .map(|x| {
                let mut v = lerp_draw_vertex(rows[at - 1][x], rows[at][x]);
                if x == boundary {
                    v.position = point;
                }
                v
            })
            .collect();
        rows.insert(at, row);
        g.height += 1;
        g.height_lod_error.insert(at, error);
    }
    // Normals are left stale: they derive from final positions only and are
    // rebuilt once per stitched grid (`refresh_normals`), not per insert.
    g.vertices = rows.into_iter().flatten().collect();
    g.stitched_vertices += 1;
}

fn refresh_normals(g: &mut PatchGrid) {
    let mut rows: Vec<_> = g.vertices.chunks_exact(g.width).map(|r| r.to_vec()).collect();
    make_patch_normals(&mut rows, g.width, g.height);
    g.vertices = rows.into_iter().flatten().collect();
}

#[cfg(test)]
fn stitch_pair(a: &PatchGrid, b: &mut PatchGrid) -> bool {
    match find_stitch(a, merged_edges(a), b) {
        Some((target, at, point, error)) => {
            insert(b, target, at, point, error);
            refresh_normals(b);
            true
        }
        None => false,
    }
}

fn merged_edges(g: &PatchGrid) -> [bool; 4] {
    std::array::from_fn(|e| merged(g, e))
}

/// The first vertex `stitch_pair` would insert into `b`: (edge, index, position, error).
/// `merged_a` is `merged_edges(a)`.
fn find_stitch(
    a: &PatchGrid,
    merged_a: [bool; 4],
    b: &PatchGrid,
) -> Option<(usize, usize, [f32; 3], f32)> {
    // Bounds of every edge. They only skip edge pairs / start points that
    // cannot contain a close vertex, so the first match is unchanged.
    let edge_box = |g: &PatchGrid, e: usize| -> [[f32; 3]; 2] {
        (0..edge_len(g, e)).fold([[f32::MAX; 3], [f32::MIN; 3]], |[lo, hi], p| {
            let v = g.vertices[edge_index(g, e, p)].position;
            [
                std::array::from_fn(|i| lo[i].min(v[i])),
                std::array::from_fn(|i| hi[i].max(v[i])),
            ]
        })
    };
    let b_boxes: [_; 4] = std::array::from_fn(|t| edge_box(b, t));
    let near = |v: [f32; 3], bx: &[[f32; 3]; 2]| bounds_may_stitch(&[v, v], bx);
    // Forward width/height, then reversed width/height, matching R_StitchPatches.
    for reversed in [false, true] {
        for edge in 0..4 {
            if merged_a[edge] {
                continue;
            }
            let a_box = edge_box(a, edge);
            if !b_boxes.iter().any(|bx| bounds_may_stitch(&a_box, bx)) {
                continue;
            }
            let len = edge_len(a, edge);
            let starts: Vec<usize> = if reversed {
                (2..len).rev().step_by(2).collect()
            } else {
                (0..len - 2).step_by(2).collect()
            };
            for k in starts {
                let end = if reversed { k - 2 } else { k + 2 };
                let mid = if reversed { k - 1 } else { k + 1 };
                for target in 0..4 {
                    if edge_len(b, target) >= OPENJK_MAX_GRID_SIZE {
                        continue;
                    }
                    if !near(a.vertices[edge_index(a, edge, k)].position, &b_boxes[target]) {
                        continue;
                    }
                    for l in 0..edge_len(b, target) - 1 {
                        let p = b.vertices[edge_index(b, target, l)].position;
                        let q = b.vertices[edge_index(b, target, l + 1)].position;
                        if !close(a.vertices[edge_index(a, edge, k)].position, p, 0.1)
                            || !close(a.vertices[edge_index(a, edge, end)].position, q, 0.1)
                            || (0..3).all(|i| f64::from((p[i] - q[i]).abs()) < 0.01)
                        {
                            continue;
                        }
                        // Upstream reversed branches use k+1 (including an
                        // out-of-bounds read). Use the actual midpoint's error;
                        // reconciliation below copies the source error again.
                        return Some((
                            target,
                            l + 1,
                            a.vertices[edge_index(a, edge, mid)].position,
                            edge_error(a, edge, mid),
                        ));
                    }
                }
            }
        }
    }
    None
}

fn grid_bounds(g: &PatchGrid) -> [[f32; 3]; 2] {
    g.vertices.iter().fold([[f32::MAX; 3], [f32::MIN; 3]], |[lo, hi], v| {
        [
            std::array::from_fn(|i| lo[i].min(v.position[i])),
            std::array::from_fn(|i| hi[i].max(v.position[i])),
        ]
    })
}

/// Conservative: grids farther apart than the stitch tolerance (0.1, padded
/// for float rounding) cannot share a close vertex, so `stitch_pair` would
/// find nothing.
fn bounds_may_stitch(a: &[[f32; 3]; 2], b: &[[f32; 3]; 2]) -> bool {
    const PAD: f32 = 0.25;
    (0..3).all(|i| a[0][i] - PAD <= b[1][i] && b[0][i] - PAD <= a[1][i])
}

fn stitch_all(grids: &mut [Option<PatchGrid>], group: &[usize]) {
    // Per-grid bounds and merged edges, refreshed whenever a grid gains a vertex.
    let mut bounds: Vec<_> = group
        .iter()
        .map(|&g| grid_bounds(grids[g].as_ref().unwrap()))
        .collect();
    let mut merged: Vec<_> = group
        .iter()
        .map(|&g| merged_edges(grids[g].as_ref().unwrap()))
        .collect();
    let mut processed = vec![false; group.len()];
    let mut stitched = vec![false; group.len()];
    loop {
        let mut visited = false;
        for (i, &source) in group.iter().enumerate() {
            if processed[i] {
                continue;
            }
            processed[i] = true;
            visited = true;
            for (j, &target) in group.iter().enumerate() {
                if !bounds_may_stitch(&bounds[i], &bounds[j]) {
                    continue;
                }
                // Include self-stitching of closed grids as the reference does.
                loop {
                    let a = grids[source].take().unwrap();
                    let changed = if source == target {
                        // Search against itself, then insert into the same grid
                        // (the reference stitches a copy; no clone needed to look).
                        let mut a = a;
                        let found = find_stitch(&a, merged[i], &a);
                        if let Some((edge, at, point, error)) = found {
                            insert(&mut a, edge, at, point, error);
                        }
                        grids[source] = Some(a);
                        found.is_some()
                    } else {
                        let b = grids[target].as_mut().unwrap();
                        let found = find_stitch(&a, merged[i], b);
                        if let Some((edge, at, point, error)) = found {
                            insert(b, edge, at, point, error);
                        }
                        grids[source] = Some(a);
                        found.is_some()
                    };
                    if !changed {
                        break;
                    }
                    stitched[j] = true;
                    bounds[j] = grid_bounds(grids[target].as_ref().unwrap());
                    merged[j] = merged_edges(grids[target].as_ref().unwrap());
                    processed[j] = false;
                }
            }
        }
        if !visited {
            break;
        }
    }
    for (i, &g) in group.iter().enumerate() {
        if stitched[i] {
            refresh_normals(grids[g].as_mut().unwrap());
        }
    }
}

fn copy_errors(a: &PatchGrid, b: &mut PatchGrid) -> bool {
    let mut touch = false;
    let merged_a = std::array::from_fn::<_, 4, _>(|e| merged(a, e));
    let merged_b = std::array::from_fn::<_, 4, _>(|e| merged(b, e));
    for e in 0..4 {
        if merged_a[e] {
            continue;
        }
        for k in 1..edge_len(a, e) - 1 {
            for t in 0..4 {
                if merged_b[t] {
                    continue;
                }
                for l in 1..edge_len(b, t) - 1 {
                    if close(
                        a.vertices[edge_index(a, e, k)].position,
                        b.vertices[edge_index(b, t, l)].position,
                        0.1,
                    ) {
                        let errors = if t < 2 {
                            &mut b.width_lod_error
                        } else {
                            &mut b.height_lod_error
                        };
                        errors[l] = edge_error(a, e, k);
                        touch = true;
                    }
                }
            }
        }
    }
    touch
}

fn fix_shared_errors(grids: &mut [Option<PatchGrid>], group: &[usize]) {
    let mut fixed = vec![false; group.len()];
    for root in 0..group.len() {
        if fixed[root] {
            continue;
        }
        fixed[root] = true;
        // Explicit stack preserves the reference's recursive depth-first
        // propagation and fixed start index without risking stack overflow.
        let start = root + 1;
        let mut stack = vec![(root, start)];
        while let Some((source, next)) = stack.last_mut() {
            if *next == group.len() {
                stack.pop();
                continue;
            }
            let target = *next;
            *next += 1;
            if fixed[target] {
                continue;
            }
            let source_grid = grids[group[*source]].take().unwrap();
            let touch = copy_errors(&source_grid, grids[group[target]].as_mut().unwrap());
            grids[group[*source]] = Some(source_grid);
            if touch {
                fixed[target] = true;
                stack.push((target, start));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributes(position: [f32; 3], x: usize, y: usize, regular: bool) -> Vertex {
        Vertex {
            position,
            texcoord: [x as f32 * 0.25, y as f32 * 0.125],
            normal: [0.0, 0.0, 1.0],
            lightmap_uv: std::array::from_fn(|l| {
                [
                    x as f32 * 0.0625 + if regular { l as f32 * 0.125 } else { 0.0 },
                    y as f32 * 0.0625,
                ]
            }),
            color: std::array::from_fn(|l| {
                std::array::from_fn(|c| ((x * 17 + y * 11 + l * 13 + c * 7) % 256) as u8)
            }),
        }
    }

    fn controls(kind: usize) -> (usize, usize, Vec<Vertex>) {
        let w = match kind {
            0 | 2 => 5,
            1 => 3,
            _ => 9,
        };
        let h = if kind == 2 { 7 } else { 3 };
        let xs = [1, 1, 0, -1, -1, -1, 0, 1, 1];
        let ys = [0, 1, 1, 1, 0, -1, -1, -1, 0];
        let mut vertices = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let position = if kind == 3 {
                    [xs[x] as f32 * 128.0, ys[x] as f32 * 128.0, y as f32 * 64.0]
                } else {
                    let z = if kind == 1 && x == 1 && y == 1 {
                        512.0
                    } else if kind == 2 {
                        (x * x * 11 + y * y * 19 + (x % 2) * 85) as f32 - (y % 2) as f32 * 33.0
                    } else {
                        0.0
                    };
                    [x as f32 * 32.0, y as f32 * 32.0, z]
                };
                vertices.push(attributes(position, x, y, true));
            }
        }
        (w, h, vertices)
    }

    fn stitch_fixture(orientation: usize) -> Vec<Option<PatchGrid>> {
        (0..2)
            .map(|grid| {
                let w = if grid == 0 {
                    if orientation & 2 != 0 {
                        3
                    } else {
                        5
                    }
                } else {
                    3
                };
                let h = if grid == 0 && orientation & 2 != 0 {
                    5
                } else {
                    3
                };
                let mut vertices = Vec::new();
                for y in 0..h {
                    for x in 0..w {
                        let mut px = (x * if grid == 0 { 1 } else { 2 }) as f32;
                        let mut py = y as f32 + if grid == 0 { 0.0 } else { -2.0 };
                        if grid == 0 && orientation & 2 != 0 {
                            px = y as f32;
                            py = x as f32;
                        }
                        if grid == 0 && orientation & 1 != 0 {
                            px = 4.0 - px;
                        }
                        vertices.push(attributes([px, py, 0.0], x, y, false));
                    }
                }
                Some(PatchGrid {
                    width: w,
                    height: h,
                    vertices,
                    subdivision_error: 4.0,
                    stitched_vertices: 0,
                    width_lod_error: (0..w)
                        .map(|i| {
                            if i == 0 || i == w - 1 {
                                0.0
                            } else {
                                i as f32
                                    * if grid == 0 {
                                        if orientation & 2 != 0 {
                                            0.5
                                        } else {
                                            1.0
                                        }
                                    } else {
                                        0.2
                                    }
                            }
                        })
                        .collect(),
                    height_lod_error: (0..h)
                        .map(|i| {
                            if i == 0 || i == h - 1 {
                                0.0
                            } else {
                                i as f32
                                    * if grid == 0 && orientation & 2 != 0 {
                                        1.0
                                    } else {
                                        0.5
                                    }
                            }
                        })
                        .collect(),
                })
            })
            .collect()
    }

    fn approx(a: f32, b: f32, context: &str) {
        assert!(
            (a - b).abs() <= 3.0e-5 * (1.0 + b.abs()),
            "{context}: Rust={a}, native={b}"
        );
    }

    /// Golden output is generated by compiling the actual TaystJK functions
    /// with scripts/verify-patch-lod-reference.py. No C++ toolchain needed in CI.
    #[test]
    fn grids_attributes_stitching_and_distance_match_native_taystjk() {
        let mut lines = include_str!("../../tests/data/patch_lod_reference.txt").lines();
        let mut cases = 0;
        while let Some(header) = lines.next() {
            let fields: Vec<_> = header.split_whitespace().collect();
            assert_eq!(fields[0], "G");
            let kind: usize = fields[1].parse().unwrap();
            let sub: f32 = fields[2].parse().unwrap();
            let grid = if kind < 10 {
                let (w, h, controls) = controls(kind);
                let mut g = subdivide_patch_to_grid(w, h, &controls, sub).unwrap();
                orient_grid(&mut g);
                g
            } else {
                let mut grids = stitch_fixture((kind - 10) / 2);
                if sub != 0.0 {
                    stitch_all(&mut grids, &[0, 1]);
                }
                fix_shared_errors(&mut grids, &[0, 1]);
                grids[(kind - 10) % 2].take().unwrap()
            };
            assert_eq!(
                grid.width,
                fields[3].parse::<usize>().unwrap(),
                "case {kind}/{sub}"
            );
            assert_eq!(
                grid.height,
                fields[4].parse::<usize>().unwrap(),
                "case {kind}/{sub}"
            );
            for errors in [&grid.width_lod_error, &grid.height_lod_error] {
                let expected: Vec<f32> = lines
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .map(|f| f.parse().unwrap())
                    .collect();
                assert_eq!(errors.len(), expected.len());
                for (&a, &b) in errors.iter().zip(&expected) {
                    approx(a, b, "LOD error");
                }
            }
            for vertex in &grid.vertices {
                let expected: Vec<f32> = lines
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .map(|f| f.parse().unwrap())
                    .collect();
                let floats = vertex
                    .position
                    .into_iter()
                    .chain(vertex.texcoord)
                    .chain(vertex.lightmap_uv.into_iter().flatten())
                    .chain(vertex.normal);
                for (a, &b) in floats.zip(&expected) {
                    approx(a, b, "vertex attribute");
                }
                for (a, &b) in vertex.color.into_iter().flatten().zip(&expected[16..]) {
                    assert_eq!(a, b as u8);
                }
            }
            let lod = PatchLod::from_grid(&grid, [[0.0, 16.0, 0.0], [0.0, 16.0, 64.0]]);
            for _ in 0..20 {
                let expected: Vec<_> = lines.next().unwrap().split_whitespace().collect();
                assert_eq!(expected[0], "L");
                let err = expected[1].parse().unwrap();
                let view: usize = expected[2].parse().unwrap();
                let origin = [
                    match view {
                        0 => 1.0,
                        1 => 4096.0,
                        2 => -4096.0,
                        _ => 0.0,
                    },
                    0.0,
                    0.0,
                ];
                let forward = if view == 3 {
                    [0.0, 1.0, 0.0]
                } else {
                    [1.0, 0.0, 0.0]
                };
                let threshold = lod.threshold(origin, forward, err);
                approx(threshold, expected[3].parse().unwrap(), "view threshold");
                let masks = lod.selection(threshold);
                assert_eq!(masks[0].count_ones(), expected[4].parse::<u32>().unwrap());
                assert_eq!(masks[1].count_ones(), expected[5].parse::<u32>().unwrap());
                let mut indices = Vec::new();
                lod.append_indices(
                    masks,
                    &(0..(grid.width * grid.height) as u32).collect::<Vec<_>>(),
                    &mut indices,
                );
                assert_eq!(
                    indices.len(),
                    (masks[0].count_ones() - 1) as usize * (masks[1].count_ones() - 1) as usize * 6
                );
                for triangle in indices.chunks_exact(3) {
                    assert!(triangle
                        .iter()
                        .all(|&i| i < (grid.width * grid.height) as u32));
                }
            }
            assert_eq!(lines.next(), Some("END"));
            cases += 1;
        }
        assert_eq!(cases, 36);
    }

    #[test]
    fn boundaries_and_65th_mask_bit_survive_extreme_lod() {
        let lod = PatchLod {
            width: 65,
            height: 3,
            width_lod_error: vec![1.0; 65],
            height_lod_error: vec![1.0; 3],
            lod_origin: [0.0; 3],
            lod_radius: 0.0,
            subdivision_error: 0.0,
            stitched_vertices: 0,
        };
        assert_eq!(lod.selection(0.0), [1 | (1 << 64), 1 | (1 << 2)]);
        let mut indices = Vec::new();
        lod.append_indices(
            lod.selection(0.0),
            &(0..195).collect::<Vec<_>>(),
            &mut indices,
        );
        assert_eq!(indices, [0, 64, 130, 64, 194, 130]);
        assert_eq!(lod.selection(1.0)[0].count_ones(), 65);
    }

    #[test]
    fn merged_interiors_are_not_stitched_or_reconciled() {
        let mut grids = stitch_fixture(0);
        let a = grids[0].as_mut().unwrap();
        a.vertices[3].position = a.vertices[1].position;
        assert!(merged(a, 0));
        let b = grids[1].as_ref().unwrap();
        assert_eq!(b.width, 3);
        // Other source edges are disjoint from the target.
        assert!(!stitch_pair(
            grids[0].as_ref().unwrap(),
            &mut stitch_fixture(0)[1].take().unwrap()
        ));
    }
}
