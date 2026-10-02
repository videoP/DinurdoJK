//! Port of OpenJK's `R_MarkFragments` world projection (tr_marks.cpp).
//!
//! Stock marks (saber burns, blob shadows, impact decals) are never drawn where
//! a collision trace landed; they are *projected onto the drawn world surfaces*
//! and clipped to them. That has three consequences the collision trace alone
//! cannot reproduce, all of which this module restores:
//!
//! * a surface whose shader says `surfaceparm nomarks` / `noimpact`, or whose
//!   contents are fog, is never marked;
//! * `SF_TRIANGLES` surfaces are never marked, because the stock
//!   `r_marksOnTriangleMeshes` cvar defaults to 0. Compiled-in MD3 geometry
//!   (q3map2 `misc_model`) is triangle soup, so models get no marks;
//! * with no drawn world surface under the hit (a hidden clip/caulk brush, an
//!   entity model's collision hull) nothing is produced at all.
//!
//! Only static world model 0 is considered, like `R_BoxSurfaces_r`.
use super::{Bsp, Mesh, SurfaceKind};

pub const SURF_NOIMPACT: u32 = 0x0008_0000;
pub const SURF_NOMARKS: u32 = 0x0010_0000;
pub const CONTENTS_FOG: u32 = 0x0000_0008;

/// `MAX_VERTS_ON_POLY`: a clip that could overflow is discarded, as in the engine.
const MAX_VERTS_ON_POLY: usize = 64;
/// Edge length of the broad-phase cells, in map units.
const CELL: f32 = 128.0;
/// Triangles spanning more cells than this (big floors) live in a short linear list.
const MAX_CELLS_PER_TRIANGLE: i64 = 48;
/// `R_AddMarkFragments` chops with this plane epsilon.
const CHOP_EPSILON: f32 = 0.5;
/// CG_ImpactMark / CG_CreateSaberMarks: `MAX_MARK_FRAGMENTS` and `MAX_MARK_POINTS`.
pub const MAX_MARK_FRAGMENTS: usize = 128;
pub const MAX_MARK_POINTS: usize = 384;

#[derive(Clone, Copy, Debug)]
struct MarkTriangle {
    points: [[f32; 3]; 3],
    mins: [f32; 3],
    maxs: [f32; 3],
    /// Plane normal used for the facing test against the projection direction.
    normal: [f32; 3],
    /// The triangle is skipped unless `dot(normal, dir) <= -min_facing`.
    min_facing: f32,
}

/// Immutable, cheap-to-clone (`Arc` it) markable-surface index of one map.
#[derive(Debug, Default)]
pub struct MarkSurfaces {
    triangles: Vec<MarkTriangle>,
    /// Sorted `(cell key, triangle index)` broad-phase entries.
    cells: Vec<(u64, u32)>,
    /// Triangles too large for the cell grid; tested by bounds on every query.
    large: Vec<u32>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length <= 1.0e-8 {
        [0.0; 3]
    } else {
        [v[0] / length, v[1] / length, v[2] / length]
    }
}

fn cell_coord(value: f32) -> i64 {
    (value / CELL).floor() as i64
}

fn cell_key(x: i64, y: i64, z: i64) -> u64 {
    const BIAS: i64 = 1 << 20;
    const MASK: u64 = (1 << 21) - 1;
    (((x + BIAS) as u64 & MASK) << 42) | (((y + BIAS) as u64 & MASK) << 21) | ((z + BIAS) as u64 & MASK)
}

fn bounds_of(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut mins = [f32::INFINITY; 3];
    let mut maxs = [f32::NEG_INFINITY; 3];
    for point in points {
        for axis in 0..3 {
            mins[axis] = mins[axis].min(point[axis]);
            maxs[axis] = maxs[axis].max(point[axis]);
        }
    }
    (mins, maxs)
}

impl MarkTriangle {
    fn new(points: [[f32; 3]; 3], normal: [f32; 3], min_facing: f32) -> Self {
        let (mins, maxs) = bounds_of(&points);
        Self { points, mins, maxs, normal, min_facing }
    }
}

/// One clipped mark polygon: a convex fan of `points` (a range into
/// [`MarkBuffer::points`]) lying on a single world triangle with `normal`.
#[derive(Clone, Debug)]
pub struct MarkFragment {
    pub points: std::ops::Range<usize>,
    pub normal: [f32; 3],
}

/// Reusable output and scratch space for [`MarkSurfaces::project`], so a caller
/// that projects every frame allocates nothing once warm.
#[derive(Default)]
pub struct MarkBuffer {
    pub points: Vec<[f32; 3]>,
    pub fragments: Vec<MarkFragment>,
    candidates: Vec<u32>,
    ping: Vec<[f32; 3]>,
    pong: Vec<[f32; 3]>,
}

impl MarkBuffer {
    pub fn clear(&mut self) {
        self.points.clear();
        self.fragments.clear();
    }

    /// Each fragment's world points and surface normal.
    pub fn iter(&self) -> impl Iterator<Item = (&[[f32; 3]], [f32; 3])> {
        self.fragments.iter().map(|fragment| (&self.points[fragment.points.clone()], fragment.normal))
    }
}

impl MarkSurfaces {
    /// Index of plain world triangles (each with its outward normal), treated
    /// like `SF_FACE`. Used when there is no BSP surface table to read flags
    /// from, such as a source-.map preview or a test.
    pub fn from_world_triangles(triangles: impl IntoIterator<Item = ([[f32; 3]; 3], [f32; 3])>) -> Self {
        Self::from_triangles(
            triangles
                .into_iter()
                .map(|(points, normal)| MarkTriangle::new(points, normal, 0.5))
                .collect(),
        )
    }

    /// `shader_flags[i]` is the `(surfaceFlags, contents)` of BSP shader `i` as the
    /// *runtime* shader reports them: `R_MarkFragments` reads `surf->shader`, so
    /// the script's `surfaceparm`s win over the compiled lump when a script exists.
    pub fn from_bsp(bsp: &Bsp, mesh: &Mesh, shader_flags: &[(u32, u32)]) -> Self {
        let mut triangles = Vec::new();
        for batch in &mesh.batches {
            let Some(surface) = bsp.surfaces.get(batch.surface) else { continue };
            let (surface_flags, contents) = shader_flags
                .get(batch.shader)
                .copied()
                .or_else(|| bsp.shaders.get(batch.shader).map(|s| (s.surface_flags, s.contents)))
                .unwrap_or((0, 0));
            if surface_flags & (SURF_NOIMPACT | SURF_NOMARKS) != 0 || contents & CONTENTS_FOG != 0 {
                continue;
            }
            // SF_TRIANGLES is only marked when r_marksOnTriangleMeshes is set;
            // it defaults to 0 in OpenJK and JKA.
            let planar = match surface.kind {
                SurfaceKind::Planar => true,
                SurfaceKind::Patch => false,
                SurfaceKind::Triangles | SurfaceKind::Flare => continue,
            };
            let Some(indices) = mesh.indices.get(batch.indices.clone()) else { continue };
            let face_normal = if planar {
                let mut sum = [0.0; 3];
                for index in indices {
                    if let Some(vertex) = mesh.vertices.get(*index as usize) {
                        sum = [sum[0] + vertex.normal[0], sum[1] + vertex.normal[1], sum[2] + vertex.normal[2]];
                    }
                }
                normalize(sum)
            } else {
                [0.0; 3]
            };
            for (n, tri) in indices.chunks_exact(3).enumerate() {
                let (Some(a), Some(b), Some(c)) = (
                    mesh.vertices.get(tri[0] as usize),
                    mesh.vertices.get(tri[1] as usize),
                    mesh.vertices.get(tri[2] as usize),
                ) else {
                    continue;
                };
                let points = [a.position, b.position, c.position];
                if planar {
                    // SF_FACE: `DotProduct(plane.normal, dir) > -0.5` is skipped.
                    triangles.push(MarkTriangle::new(points, face_normal, 0.5));
                } else {
                    // SF_GRID triangulates each quad as (v0, v[w], v1) then
                    // (v1, v[w], v[w+1]); the mesh keeps that winding and order.
                    // The engine tests the triangle's own normal, -0.1 then -0.05.
                    let normal = normalize(cross(sub(points[0], points[1]), sub(points[2], points[1])));
                    triangles.push(MarkTriangle::new(points, normal, if n % 2 == 0 { 0.1 } else { 0.05 }));
                }
            }
        }
        Self::from_triangles(triangles)
    }

    fn from_triangles(triangles: Vec<MarkTriangle>) -> Self {
        let mut cells = Vec::new();
        let mut large = Vec::new();
        for (index, triangle) in triangles.iter().enumerate() {
            if !triangle.mins[0].is_finite() || !triangle.maxs[0].is_finite() {
                continue;
            }
            let (lo, hi) = (triangle.mins.map(cell_coord), triangle.maxs.map(cell_coord));
            let count = (0..3).map(|axis| hi[axis] - lo[axis] + 1).product::<i64>();
            if count > MAX_CELLS_PER_TRIANGLE {
                large.push(index as u32);
                continue;
            }
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        cells.push((cell_key(x, y, z), index as u32));
                    }
                }
            }
        }
        cells.sort_unstable();
        Self { triangles, cells, large }
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// `R_MarkFragments` for a quad (`points`, wound as `CG_ImpactMark` /
    /// `CG_CreateSaberMarks` build it) projected along `projection`. The result
    /// replaces the contents of `out`: convex fans clipped to drawn, markable
    /// triangles, capped at the stock fragment and point limits.
    pub fn project(&self, points: &[[f32; 3]; 4], projection: [f32; 3], out: &mut MarkBuffer) {
        out.clear();
        if self.triangles.is_empty() {
            return;
        }
        let dir = normalize(projection);
        if dir == [0.0; 3] {
            return;
        }

        // Broad-phase box. The clip planes below are the authority, so this just
        // bounds what they can admit: the near plane keeps points up to 31.5 units
        // behind the quad (against `dir`) and the far plane up to 19.5 ahead of it.
        // Stock finds surfaces by BSP leaf, which is coarser than its own tight box.
        let mut pts = [[0.0f32; 3]; 12];
        for (i, point) in points.iter().enumerate() {
            pts[i * 3] = *point;
            pts[i * 3 + 1] = [point[0] - dir[0] * 32.0, point[1] - dir[1] * 32.0, point[2] - dir[2] * 32.0];
            pts[i * 3 + 2] = [point[0] + dir[0] * 20.0, point[1] + dir[1] * 20.0, point[2] + dir[2] * 20.0];
        }
        let (mins, maxs) = bounds_of(&pts);

        // Bounding planes of the projected prism, then near and far planes.
        let mut normals = [[0.0f32; 3]; 6];
        let mut dists = [0.0f32; 6];
        for i in 0..4 {
            let v1 = sub(points[(i + 1) % 4], points[i]);
            let v2 = [-projection[0], -projection[1], -projection[2]];
            normals[i] = normalize(cross(v1, v2));
            dists[i] = dot(normals[i], points[i]);
        }
        normals[4] = dir;
        dists[4] = dot(dir, points[0]) - 32.0;
        normals[5] = [-dir[0], -dir[1], -dir[2]];
        dists[5] = dot(normals[5], points[0]) - 20.0;

        let candidates = &mut out.candidates;
        candidates.clear();
        let (lo, hi) = (mins.map(cell_coord), maxs.map(cell_coord));
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let key = cell_key(x, y, z);
                    let start = self.cells.partition_point(|entry| entry.0 < key);
                    candidates.extend(self.cells[start..].iter().take_while(|entry| entry.0 == key).map(|entry| entry.1));
                }
            }
        }
        candidates.extend_from_slice(&self.large);
        candidates.sort_unstable();
        candidates.dedup();

        for &index in out.candidates.iter() {
            let triangle = &self.triangles[index as usize];
            if dot(triangle.normal, dir) > -triangle.min_facing
                || (0..3).any(|axis| triangle.maxs[axis] < mins[axis] || triangle.mins[axis] > maxs[axis])
            {
                continue;
            }
            out.ping.clear();
            out.ping.extend_from_slice(&triangle.points);
            for plane in 0..6 {
                chop_poly_behind_plane(&out.ping, &mut out.pong, normals[plane], dists[plane], CHOP_EPSILON);
                std::mem::swap(&mut out.ping, &mut out.pong);
                if out.ping.is_empty() {
                    break;
                }
            }
            if out.ping.len() < 3 {
                continue;
            }
            // R_AddMarkFragments: a polygon that does not fit is dropped.
            if out.points.len() + out.ping.len() > MAX_MARK_POINTS {
                continue;
            }
            let start = out.points.len();
            out.points.extend_from_slice(&out.ping);
            out.fragments.push(MarkFragment { points: start..out.points.len(), normal: triangle.normal });
            if out.fragments.len() >= MAX_MARK_FRAGMENTS {
                break;
            }
        }
    }

    /// True when a small mark centred on `point` (surface normal `normal`) would
    /// land on at least one drawn, markable surface.
    pub fn has_markable_surface(&self, point: [f32; 3], normal: [f32; 3]) -> bool {
        let normal = normalize(normal);
        if normal == [0.0; 3] {
            return false;
        }
        let helper = if normal[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
        // Same handedness as CG_CreateSaberMarks: axis2 = cross(axis1, normal).
        let u = normalize(cross(normal, helper));
        let v = cross(u, normal);
        let r = 0.65;
        let corner = |su: f32, sv: f32| {
            [
                point[0] + u[0] * r * su + v[0] * r * sv,
                point[1] + u[1] * r * su + v[1] * r * sv,
                point[2] + u[2] * r * su + v[2] * r * sv,
            ]
        };
        let quad = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
        {
            let mut out = MarkBuffer::default();
            self.project(&quad, [-normal[0], -normal[1], -normal[2]], &mut out);
            !out.fragments.is_empty()
        }
    }
}

/// `R_ChopPolyBehindPlane`: keep the part of `input` in front of the plane.
fn chop_poly_behind_plane(input: &[[f32; 3]], out: &mut Vec<[f32; 3]>, normal: [f32; 3], dist: f32, epsilon: f32) {
    out.clear();
    // Don't clip if it might overflow.
    if input.len() >= MAX_VERTS_ON_POLY - 2 {
        return;
    }
    const FRONT: u8 = 0;
    const BACK: u8 = 1;
    const ON: u8 = 2;
    let mut counts = [0usize; 3];
    let mut dists = [0.0f32; MAX_VERTS_ON_POLY + 4];
    let mut sides = [0u8; MAX_VERTS_ON_POLY + 4];
    for (i, point) in input.iter().enumerate() {
        let d = dot(*point, normal) - dist;
        dists[i] = d;
        sides[i] = if d > epsilon {
            FRONT
        } else if d < -epsilon {
            BACK
        } else {
            ON
        };
        counts[sides[i] as usize] += 1;
    }
    sides[input.len()] = sides[0];
    dists[input.len()] = dists[0];

    if counts[FRONT as usize] == 0 {
        return;
    }
    if counts[BACK as usize] == 0 {
        out.extend_from_slice(input);
        return;
    }
    for i in 0..input.len() {
        let p1 = input[i];
        if sides[i] == ON {
            out.push(p1);
            continue;
        }
        if sides[i] == FRONT {
            out.push(p1);
        }
        if sides[i + 1] == ON || sides[i + 1] == sides[i] {
            continue;
        }
        // Generate a split point.
        let p2 = input[(i + 1) % input.len()];
        let d = dists[i] - dists[i + 1];
        let t = if d == 0.0 { 0.0 } else { dists[i] / d };
        out.push([
            p1[0] + t * (p2[0] - p1[0]),
            p1[1] + t * (p2[1] - p1[1]),
            p1[2] + t * (p2[2] - p1[2]),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(points: [[f32; 3]; 3]) -> ([[f32; 3]; 3], [f32; 3]) {
        // Orient the normal toward +Z; winding is irrelevant to the clipper.
        let mut normal = normalize(cross(sub(points[1], points[0]), sub(points[2], points[0])));
        if normal[2] < 0.0 {
            normal = normal.map(|v| -v);
        }
        (points, normal)
    }

    fn floor() -> MarkSurfaces {
        // A 512x512 floor at z = 0 facing up.
        let quad = [[-256.0, -256.0, 0.0], [256.0, -256.0, 0.0], [256.0, 256.0, 0.0], [-256.0, 256.0, 0.0]];
        MarkSurfaces::from_world_triangles([[0, 1, 2], [0, 2, 3]].map(|tri| face(tri.map(|i| quad[i]))))
    }

    /// A slash along +X over an upward face, wound as CG_CreateSaberMarks does
    /// (axis1 = +X, axis2 = cross(axis1, up) = -Y).
    fn slash(z: f32) -> [[f32; 3]; 4] {
        [[-4.0, 1.0, z], [4.0, 1.0, z], [4.0, -1.0, z], [-4.0, -1.0, z]]
    }

    /// CG_ImpactMark's blob-shadow box over an upward plane: `radius` square,
    /// projected 20 units down (so 20 below and 32 above the plane are kept).
    fn blob(radius: f32) -> ([[f32; 3]; 4], [f32; 3]) {
        (
            [[-radius, radius, 0.0], [radius, radius, 0.0], [radius, -radius, 0.0], [-radius, -radius, 0.0]],
            [0.0, 0.0, -20.0],
        )
    }

    fn project(surfaces: &MarkSurfaces, quad: &[[f32; 3]; 4], projection: [f32; 3]) -> MarkBuffer {
        let mut out = MarkBuffer::default();
        surfaces.project(quad, projection, &mut out);
        out
    }

    #[test]
    fn marks_project_onto_an_upward_face() {
        let surfaces = floor();
        let out = project(&surfaces, &slash(0.2), [0.0, 0.0, -1.0]);
        assert!(!out.fragments.is_empty());
        for (polygon, normal) in out.iter() {
            assert!(polygon.len() >= 3);
            assert_eq!(normal, [0.0, 0.0, 1.0]);
            for point in polygon {
                assert!(point[2].abs() < 1.0e-3, "fragment must lie on the face");
                assert!(point[0].abs() <= 4.5 && point[1].abs() <= 1.5, "clipped to the slash box");
            }
        }
        assert!(surfaces.has_markable_surface([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]));
    }

    #[test]
    fn faces_turned_away_from_the_projection_are_not_marked() {
        // Projecting up into the floor's back side.
        assert!(project(&floor(), &slash(-0.2), [0.0, 0.0, 1.0]).fragments.is_empty());
    }

    #[test]
    fn nothing_under_the_hit_means_no_marks() {
        let surfaces = floor();
        assert!(!surfaces.has_markable_surface([4000.0, 0.0, 0.0], [0.0, 0.0, 1.0]));
        assert!(!MarkSurfaces::default().has_markable_surface([0.0; 3], [0.0, 0.0, 1.0]));
    }

    #[test]
    fn huge_triangles_are_still_found_through_the_large_list() {
        let surfaces = MarkSurfaces::from_world_triangles([face([
            [-100_000.0, -100_000.0, 0.0],
            [100_000.0, -100_000.0, 0.0],
            [0.0, 100_000.0, 0.0],
        ])]);
        assert_eq!(surfaces.large.len(), 1);
        assert!(surfaces.has_markable_surface([10.0, 10.0, 0.0], [0.0, 0.0, 1.0]));
    }

    #[test]
    fn blob_box_clips_a_floor_to_the_shadow_square() {
        let (quad, projection) = blob(24.0);
        let out = project(&floor(), &quad, projection);
        assert!(!out.fragments.is_empty());
        let mut area = 0.0;
        for (polygon, _) in out.iter() {
            for point in polygon {
                assert!(point[0].abs() <= 24.01 && point[1].abs() <= 24.01 && point[2].abs() < 1.0e-3);
            }
            for fan in 1..polygon.len() - 1 {
                let (a, b, c) = (polygon[0], polygon[fan], polygon[fan + 1]);
                area += 0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs();
            }
        }
        assert!((area - 48.0 * 48.0).abs() < 1.0, "the square is fully covered, area {area}");
    }

    #[test]
    fn blob_box_follows_a_rug_above_the_plane_and_a_slope() {
        // A rug 1u above the plane is inside the box and marked at that height.
        let rug = MarkSurfaces::from_world_triangles([face([
            [-100.0, -100.0, 1.0],
            [100.0, -100.0, 1.0],
            [0.0, 100.0, 1.0],
        ])]);
        let (quad, projection) = blob(24.0);
        let out = project(&rug, &quad, projection);
        assert!(!out.points.is_empty());
        assert!(out.points.iter().all(|p| (p[2] - 1.0).abs() < 1.0e-4));
        // A 30-degree ramp rising along +x: marks follow it up to the +32 limit.
        let slope = 30.0f32.to_radians().tan();
        let z = |x: f32| x * slope;
        let ramp = MarkSurfaces::from_world_triangles([
            face([[-100.0, -100.0, z(-100.0)], [100.0, 100.0, z(100.0)], [100.0, -100.0, z(100.0)]]),
            face([[-100.0, -100.0, z(-100.0)], [-100.0, 100.0, z(-100.0)], [100.0, 100.0, z(100.0)]]),
        ]);
        let out = project(&ramp, &quad, projection);
        assert!(!out.fragments.is_empty());
        for p in &out.points {
            assert!((p[2] - z(p[0])).abs() < 0.5, "vertex not on the ramp: {p:?}");
        }
    }

    #[test]
    fn blob_box_skips_walls_and_geometry_outside_the_box() {
        let (quad, projection) = blob(24.0);
        // A wall (normal perpendicular to the shadow normal) is skipped.
        let wall = MarkSurfaces::from_world_triangles([face([[10.0, -50.0, -10.0], [10.0, 50.0, -10.0], [10.0, 0.0, 30.0]])]);
        assert!(project(&wall, &quad, projection).fragments.is_empty());
        // Floor 40 above (past the +32 near plane) and 30 below (past the -20 far plane).
        for z in [40.0, -30.0] {
            let floor = MarkSurfaces::from_world_triangles([face([[-50.0, -50.0, z], [50.0, -50.0, z], [0.0, 50.0, z]])]);
            assert!(project(&floor, &quad, projection).fragments.is_empty(), "floor at z={z} must not be marked");
        }
    }
}

#[cfg(test)]
mod stock_map_tests {
    use super::*;

    /// `JKA_TEST_BSP=<path to a stock .bsp> cargo test -p jka-assets stock_map_marks -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn stock_map_marks() {
        let path = std::env::var("JKA_TEST_BSP").expect("JKA_TEST_BSP");
        let bsp = Bsp::parse(&std::fs::read(path).unwrap()).unwrap();
        let mesh = bsp.world_mesh(4).unwrap();
        let built = std::time::Instant::now();
        let surfaces = MarkSurfaces::from_bsp(&bsp, &mesh, &[]);
        println!("index build {:?}", built.elapsed());
        let (mut faces, mut patches, mut soup) = (0, 0, 0);
        for batch in &mesh.batches {
            match bsp.surfaces[batch.surface].kind {
                SurfaceKind::Planar => faces += 1,
                SurfaceKind::Patch => patches += 1,
                SurfaceKind::Triangles => soup += 1,
                SurfaceKind::Flare => {}
            }
        }
        println!(
            "batches faces={faces} patches={patches} soup={soup}; markable triangles={} cells={} large={}",
            surfaces.triangles.len(),
            surfaces.cells.len(),
            surfaces.large.len()
        );
        assert!(soup > 0, "map should contain compiled-in model soup to prove the exclusion");
        let spawns = bsp.deathmatch_spawns();
        let mut hit = 0;
        for spawn in &spawns {
            let floor = [spawn.origin[0], spawn.origin[1], spawn.origin[2] - 24.0];
            if surfaces.has_markable_surface(floor, [0.0, 0.0, 1.0]) {
                hit += 1;
            }
        }
        println!("spawns with a markable floor under them: {hit}/{}", spawns.len());
        assert!(hit > 0);
        let started = std::time::Instant::now();
        let mut n = 0;
        for spawn in spawns.iter().cycle().take(20_000) {
            let floor = [spawn.origin[0], spawn.origin[1], spawn.origin[2] - 24.0];
            n += surfaces.has_markable_surface(floor, [0.0, 0.0, 1.0]) as usize;
        }
        println!("20000 queries in {:?} ({n} hits)", started.elapsed());
    }
}
