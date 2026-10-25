//! Render-only, camera-relative continuation of promoted water beyond BSP sky walls.
//! These contacts are deliberately independent from coastal and bathymetry data.
use jka_assets::bsp::bathymetry::WaterFootprint;

pub const SEGMENTS_PER_POOL: usize = 16;
const PER_SIDE: usize = SEGMENTS_PER_POOL / 4;
const TOUCH: f32 = 24.0;
const PLANE_EPSILON: f32 = 0.125;
const EDGE_EPSILON: f32 = 2.0;

/// Renderer X/Y/Z coordinates (Y up).
pub type SkyTriangle = [[f32; 3]; 3];

/// Each segment: [sky-wall coordinate, tangential minimum, tangential maximum,
/// side+1]. Side order is -X,+X,-Z,+Z; 0 means unused.
/// There is intentionally NO world-space extension distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyExtension {
    pub bounds: [f32; 4],
    pub segments: [[f32; 4]; SEGMENTS_PER_POOL],
    pub count: usize,
}

impl SkyExtension {
    pub fn contains(&self, p: [f32; 2]) -> bool {
        let [min_x, min_z, max_x, max_z] = self.bounds;
        for segment in self.segments.iter().take(self.count) {
            let [wall, lo, hi, side] = *segment;
            match side as usize {
                1 if p[0] <= wall + EDGE_EPSILON && p[1] >= lo && p[1] <= hi => return true,
                2 if p[0] >= wall - EDGE_EPSILON && p[1] >= lo && p[1] <= hi => return true,
                3 if p[1] <= wall + EDGE_EPSILON && p[0] >= lo && p[0] <= hi => return true,
                4 if p[1] >= wall - EDGE_EPSILON && p[0] >= lo && p[0] <= hi => return true,
                _ => {}
            }
        }
        // At a corner, two sky walls opening onto the same water body must
        // admit the entire outside quadrant, not two strips meeting at a point.
        // Require BOTH contacted edges to touch the actual shared corner.
        let x_side = if p[0] < min_x { Some(1) } else if p[0] > max_x { Some(2) } else { None };
        let z_side = if p[1] < min_z { Some(3) } else if p[1] > max_z { Some(4) } else { None };
        if let (Some(xs), Some(zs)) = (x_side, z_side) {
            let x_corner = if xs == 1 { min_x } else { max_x };
            let z_corner = if zs == 3 { min_z } else { max_z };
            let touches = |side, tangent: f32| self.segments.iter().take(self.count).any(|s| {
                s[3] as usize == side && tangent >= s[1] - EDGE_EPSILON && tangent <= s[2] + EDGE_EPSILON
            });
            return touches(xs, z_corner) && touches(zs, x_corner);
        }
        false
    }

    /// Conservative CPU test used only to prune provably dry clipmap cells.
    /// The GPU does the precise water outline and sky contact test.
    pub fn rect_may_contain(&self, lo: [f32; 2], hi: [f32; 2]) -> bool {
        for segment in self.segments.iter().take(self.count) {
            let [wall, a, b, side] = *segment;
            let possible = match side as usize {
                1 => lo[0] <= wall + EDGE_EPSILON && hi[1] >= a && lo[1] <= b,
                2 => hi[0] >= wall - EDGE_EPSILON && hi[1] >= a && lo[1] <= b,
                3 => lo[1] <= wall + EDGE_EPSILON && hi[0] >= a && lo[0] <= b,
                4 => hi[1] >= wall - EDGE_EPSILON && hi[0] >= a && lo[0] <= b,
                _ => false,
            };
            if possible { return true; }
        }
        // A rectangle can lie completely in an open corner, outside both
        // strips. Sample the nearest quadrant point, not the distant corners.
        let [min_x, min_z, max_x, max_z] = self.bounds;
        for &x in &[min_x - EDGE_EPSILON*2.0, max_x + EDGE_EPSILON*2.0] {
            if (x < min_x && lo[0] >= min_x) || (x > max_x && hi[0] <= max_x) { continue; }
            for &z in &[min_z - EDGE_EPSILON*2.0, max_z + EDGE_EPSILON*2.0] {
                if (z < min_z && lo[1] >= min_z) || (z > max_z && hi[1] <= max_z) { continue; }
                if self.contains([x, z]) { return true; }
            }
        }
        false
    }
}

/// Slice an actual face at water level, rather than projecting its entire bounds.
/// In particular, a sky wall above a low retaining wall is not water contact.
fn waterline(triangle: &SkyTriangle, height: f32) -> Option<[[f32; 2]; 2]> {
    let mut points = [[0.0; 2]; 6];
    let mut count = 0;
    for i in 0..3 {
        let a = triangle[i];
        let b = triangle[(i + 1) % 3];
        let da = a[1] - height;
        let db = b[1] - height;
        if da.abs() <= PLANE_EPSILON {
            points[count] = [a[0], a[2]];
            count += 1;
        }
        if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
            let t = da / (da - db);
            points[count] = [a[0] + t * (b[0] - a[0]), a[2] + t * (b[2] - a[2])];
            count += 1;
        }
    }
    let mut longest = 0.0;
    let mut result = None;
    for a in &points[..count] {
        for b in &points[..count] {
            let length = (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
            if length > longest {
                longest = length;
                result = Some([*a, *b]);
            }
        }
    }
    result
}

/// Tangential extent of a line segment clipped to a strip along a pool edge.
fn strip_interval(line: [[f32; 2]; 2], axis: usize, lo: f32, hi: f32) -> Option<[f32; 2]> {
    let [a, b] = line;
    let delta = b[axis] - a[axis];
    let (start, end) = if delta.abs() < 1e-6 {
        if a[axis] < lo || a[axis] > hi { return None; }
        (0.0, 1.0)
    } else {
        let x = (lo - a[axis]) / delta;
        let y = (hi - a[axis]) / delta;
        (x.min(y).max(0.0), x.max(y).min(1.0))
    };
    if start > end { return None; }
    let tangent = 1 - axis;
    let x = a[tangent] + start * (b[tangent] - a[tangent]);
    let y = a[tangent] + end * (b[tangent] - a[tangent]);
    Some([x.min(y), x.max(y)])
}

/// Discover vertical, axis-aligned sky walls contacting REAL water triangles.
/// Opaque world faces at the seam disqualify only the spans they actually close.
/// No synthetic water mesh, atlas, bathymetry, or fixed-radius strips are built.
pub fn detect(original: &WaterFootprint, sky_triangles: &[SkyTriangle], blockers: &[SkyTriangle]) -> Option<SkyExtension> {
    if original.triangles.is_empty() || !original.plane.is_finite() { return None; }
    let mut result = SkyExtension {
        bounds: [original.minimum[0], original.minimum[1], original.maximum[0], original.maximum[1]],
        segments: [[0.0; 4]; SEGMENTS_PER_POOL], count: 0,
    };
    for side in 0..4 {
        let axis = side / 2;
        let tangent = 1 - axis;
        let outward_min = side % 2 == 0;
        let edge = if outward_min { original.minimum[axis] } else { original.maximum[axis] };
        let contacts: Vec<[f32;2]> = original.triangles.iter().filter_map(|water| {
            let e = if outward_min {
                water.iter().map(|v| v[axis]).fold(f32::INFINITY, f32::min)
            } else {
                water.iter().map(|v| v[axis]).fold(f32::NEG_INFINITY, f32::max)
            };
            if (e-edge).abs() > TOUCH { return None; }
            // A triangle with one vertex at the boundary does not contact sky
            // along its entire projected width. Clip its edges at the seam.
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for i in 0..3 {
                if let Some(span) = strip_interval([water[i], water[(i+1)%3]], axis,
                    edge - EDGE_EPSILON, edge + EDGE_EPSILON) {
                    lo = lo.min(span[0]);
                    hi = hi.max(span[1]);
                }
            }
            (hi - lo >= 1.0).then_some([lo, hi])
        }).collect();
        if contacts.is_empty() { continue; }
        let mut intervals = Vec::<[f32;2]>::new();
        let mut seam_lo = edge;
        let mut seam_hi = edge;
        for sky in sky_triangles {
            if sky.iter().all(|v| (v[1] - sky[0][1]).abs() <= PLANE_EPSILON) { continue; }
            let Some(line) = waterline(sky, original.plane) else { continue; };
            let coord = |v: &[f32;3], a: usize| v[if a == 0 { 0 } else { 2 }];
            let wall_lo = sky.iter().map(|v| coord(v, axis)).fold(f32::INFINITY,f32::min);
            let wall_hi = sky.iter().map(|v| coord(v, axis)).fold(f32::NEG_INFINITY,f32::max);
            if wall_hi-wall_lo > TOUCH || (0.5*(wall_lo+wall_hi)-edge).abs() > TOUCH { continue; }
            let tang_lo = line[0][tangent].min(line[1][tangent]);
            let tang_hi = line[0][tangent].max(line[1][tangent]);
            if tang_hi-tang_lo < 1.0 { continue; }
            for contact in &contacts {
                let lo = tang_lo.max(contact[0]);
                let hi = tang_hi.min(contact[1]);
                if hi-lo >= 1.0 {
                    intervals.push([lo,hi]);
                    seam_lo = seam_lo.min(line[0][axis]).min(line[1][axis]);
                    seam_hi = seam_hi.max(line[0][axis]).max(line[1][axis]);
                }
            }
        }
        intervals.sort_by(|a,b| a[0].total_cmp(&b[0]));
        let mut merged = Vec::<[f32;2]>::new();
        for interval in intervals {
            if let Some(last) = merged.last_mut() {
                if interval[0] <= last[1] + 8.0 {
                    last[1] = last[1].max(interval[1]);
                    continue;
                }
            }
            merged.push(interval);
        }
        // Expand before removing solid spans, so edge padding and merging can
        // never re-open a wall we just rejected. Only inspect the actual seam
        // between water and sky, not nearby rocks inside or beyond the pool.
        for interval in &mut merged {
            interval[0] = (interval[0] - EDGE_EPSILON).max(original.minimum[tangent]);
            interval[1] = (interval[1] + EDGE_EPSILON).min(original.maximum[tangent]);
        }
        let strip_lo = seam_lo - PLANE_EPSILON;
        let strip_hi = seam_hi + PLANE_EPSILON;
        for blocker in blockers {
            if merged.is_empty() { break; }
            let Some(line) = waterline(blocker, original.plane) else { continue; };
            let Some([lo, hi]) = strip_interval(line, axis, strip_lo, strip_hi) else { continue; };
            if hi - lo < PLANE_EPSILON { continue; }
            merged = merged.into_iter().flat_map(|[a, b]| {
                if hi <= a || lo >= b { return vec![[a,b]]; }
                let mut pieces = Vec::with_capacity(2);
                if lo - a >= 1.0 { pieces.push([a,lo]); }
                if b - hi >= 1.0 { pieces.push([hi,b]); }
                pieces
            }).collect();
        }
        merged.sort_by(|a,b| (b[1]-b[0]).total_cmp(&(a[1]-a[0])));
        for interval in merged.into_iter().take(PER_SIDE) {
            result.segments[result.count] = [edge,
                interval[0], interval[1],
                (side+1) as f32];
            result.count += 1;
        }
    }
    (result.count > 0).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_assets::bsp::bathymetry::{build_water_footprints, WaterFace};
    fn water() -> WaterFootprint {
        build_water_footprints(
            &[WaterFace { plane: 0.0, minimum:[0.0,0.0], maximum:[100.0,100.0], authored_ocean:None, material:0 }],
            &[vec![[[0.0,0.0],[100.0,0.0],[0.0,100.0]],[[100.0,0.0],[100.0,100.0],[0.0,100.0]]]],
            256.0,
        ).remove(0)
    }
    fn wall_x(x: f32, bottom: f32, top: f32, lo: f32, hi: f32) -> [SkyTriangle; 2] {
        let a = [x,bottom,lo];
        let b = [x,top,lo];
        let c = [x,bottom,hi];
        let d = [x,top,hi];
        [[a,b,c],[b,d,c]]
    }
    #[test]
    fn camera_can_travel_arbitrarily_far_outside_a_sky_wall() {
        let sky = detect(&water(), &wall_x(100.0,-10.0,100.0,0.0,100.0), &[]).unwrap();
        assert!(sky.contains([1_000_000.0, 50.0]));
        assert!(!sky.contains([-100.0, 50.0]));
        assert!(!sky.contains([1_000_000.0, 150.0]));
    }
    #[test]
    fn touching_sky_walls_fill_the_outer_corner() {
        let mut walls = wall_x(100.0,-10.0,100.0,0.0,100.0).to_vec();
        walls.extend(wall_x(100.0,-10.0,100.0,0.0,100.0).map(|t| t.map(|[x,y,z]| [z,y,x])));
        let sky = detect(&water(), &walls, &[]).unwrap();
        assert!(sky.contains([100000.0,100000.0]));
        assert!(sky.rect_may_contain([10000.0,10000.0],[11000.0,11000.0]));
    }
    #[test]
    fn sky_ceiling_and_distant_wall_do_not_extend() {
        assert!(detect(&water(), &[[[100.0,300.0,0.0],[100.0,300.0,100.0],[0.0,300.0,0.0]]], &[]).is_none());
        assert!(detect(&water(), &[[[100.0,0.0,0.0],[100.0,0.0,100.0],[90.0,0.0,0.0]]], &[]).is_none());
        assert!(detect(&water(), &wall_x(300.0,-10.0,100.0,0.0,100.0), &[]).is_none());
    }
    #[test]
    fn grandflawv2_twelve_unit_retaining_rim_prevents_sky_contact() {
        // The lake at viewpos (2280 -6547 -1272) is at JKA Z=-2011;
        // its PONDs_sky walls start at -1999, on top of the rock rim.
        let mut lake = water();
        lake.plane = -2011.0;
        assert!(detect(&lake, &wall_x(100.0,-1999.0,-397.0,0.0,100.0), &[]).is_none());
        assert!(detect(&lake, &wall_x(100.0,-2011.0,-397.0,0.0,100.0), &[]).is_some());
    }
    #[test]
    fn retaining_wall_blocks_sky_even_when_sky_overlaps_the_water_height() {
        let sky = wall_x(100.0,-100.0,100.0,0.0,100.0);
        let rim = wall_x(100.0,-56.0,12.0,0.0,100.0);
        assert!(detect(&water(), &sky, &rim).is_none());
        // A submerged bank and an overhead beam leave the waterline open.
        assert!(detect(&water(), &sky, &wall_x(100.0,-56.0,-1.0,0.0,100.0)).is_some());
        assert!(detect(&water(), &sky, &wall_x(100.0,1.0,12.0,0.0,100.0)).is_some());
    }
    #[test]
    fn partial_rocks_block_only_their_spans_without_bridging_small_gaps() {
        let sky = wall_x(100.0,-100.0,100.0,0.0,100.0);
        let rock = wall_x(100.0,-10.0,20.0,48.0,52.0);
        let extension = detect(&water(), &sky, &rock).unwrap();
        assert!(extension.contains([1000.0,20.0]));
        assert!(!extension.contains([1000.0,50.0]));
        assert!(extension.contains([1000.0,80.0]));
        // A rock wholly inside the pool does not seal the sky boundary.
        let inside = wall_x(90.0,-10.0,20.0,0.0,100.0);
        assert!(detect(&water(), &sky, &inside).unwrap().contains([1000.0,50.0]));
        let beyond_sky = wall_x(110.0,-10.0,20.0,0.0,100.0);
        assert!(detect(&water(), &sky, &beyond_sky).unwrap().contains([1000.0,50.0]));
        // A wall between a water edge and a sky face within the seam tolerance
        // really does separate them, even when it is not on the water edge.
        let distant_sky = wall_x(120.0,-100.0,100.0,0.0,100.0);
        assert!(detect(&water(), &distant_sky, &beyond_sky).is_none());
    }
    #[test]
    fn sloped_sky_triangle_uses_its_waterline_instead_of_its_bounds() {
        let sky = [[[100.0,-10.0,0.0],[100.0,90.0,0.0],[100.0,90.0,100.0]]];
        let extension = detect(&water(), &sky, &[]).unwrap();
        assert!(extension.contains([1000.0,5.0]));
        assert!(!extension.contains([1000.0,50.0]));
    }
    #[test]
    #[ignore = "requires target/release/base/grandflawv2.pk3 or JKA_OCEAN_SKY_MAP"]
    fn installed_grandflawv2_lake_has_no_infinite_extension() {
        use std::io::Read;
        use jka_assets::bsp::{Bsp, SurfaceKind, SURF_SKY, CONTENTS_SOLID, CONTENTS_TERRAIN};
        let path = std::env::var_os("JKA_OCEAN_SKY_MAP").map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/release/base/grandflawv2.pk3"));
        let mut pk3 = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut bytes = Vec::new();
        pk3.by_name("maps/grandflawv2.bsp").unwrap().read_to_end(&mut bytes).unwrap();
        let bsp = Bsp::parse(&bytes).unwrap();
        let mut sky = Vec::new();
        let mut blockers = Vec::new();
        let mut water_faces = Vec::new();
        let mut water_triangles = Vec::new();
        for surface in &bsp.surfaces[bsp.models[0].surfaces.clone()] {
            if !matches!(surface.kind, SurfaceKind::Planar | SurfaceKind::Triangles) { continue; }
            let shader = &bsp.shaders[surface.shader];
            let vertices = &bsp.vertices[surface.vertices.clone()];
            let triangles: Vec<SkyTriangle> = bsp.indices[surface.indices.clone()].chunks_exact(3)
                .map(|ids| [0,1,2].map(|i| {
                    let [x,y,z] = vertices[ids[i] as usize].position;
                    [x,z,-y]
                })).collect();
            if shader.surface_flags & SURF_SKY != 0 {
                sky.extend(triangles);
            } else if shader.contents & 4 != 0 && !vertices.is_empty()
                && vertices.iter().all(|v| (v.position[2] - vertices[0].position[2]).abs() < 0.01)
                && vertices.iter().any(|v| v.normal[2] > 0.0) {
                let minimum = [0,2].map(|a| triangles.iter().flatten().map(|v|v[a]).fold(f32::INFINITY,f32::min));
                let maximum = [0,2].map(|a| triangles.iter().flatten().map(|v|v[a]).fold(f32::NEG_INFINITY,f32::max));
                water_faces.push(WaterFace { plane: vertices[0].position[2], minimum, maximum,
                    authored_ocean:None, material:surface.shader as u32 });
                water_triangles.push(triangles.iter().map(|t|t.map(|v|[v[0],v[2]])).collect());
            } else if shader.contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) != 0 {
                blockers.extend(triangles);
            }
        }
        let footprints = build_water_footprints(&water_faces, &water_triangles, 256.0);
        let lake = footprints.iter().find(|p| p.plane == -2011.0 && p.minimum[0] == 1383.0).unwrap();
        assert_eq!(lake.minimum, [1383.0,723.75]);
        assert_eq!(lake.maximum, [3399.0,8402.25]);
        assert!(detect(lake, &sky, &blockers).is_none());
        let brushes = crate::ocean::volume::from_bsp(&bsp);
        let bottom = crate::ocean::volume::promoted_bottom(lake, &brushes);
        // The main lake is 56 units deep at the reported view position, but
        // includes a connected deeper well elsewhere. Keep its conservative
        // lower bound instead of clipping off that legitimate water pocket.
        assert_eq!(bottom, -2287.0);
        let local_bottom = brushes.iter().filter(|b| b.maximum[1] == lake.plane
            && b.minimum[0] <= 1725.0 && b.maximum[0] >= 1725.0
            && b.minimum[2] <= 6485.0 && b.maximum[2] >= 6485.0)
            .map(|b| b.minimum[1]).fold(lake.plane, f32::min);
        assert_eq!(local_bottom, -2067.0);
        let volume = crate::ocean::WaterVolume {
            minimum:[lake.minimum[0],bottom,lake.minimum[1]],
            maximum:[lake.maximum[0],lake.plane,lake.maximum[1]], mask_slot:None,
        };
        let masks = crate::ocean::OceanMasks::default();
        assert!(volume.contains([1725.0,-2040.0,6485.0], &masks));
        assert!(!volume.contains([1725.0,-3273.0,6485.0], &masks));
        println!("Grandflawv2 lake: height={}, bounds={:?}..{:?}, no sky extension",
            lake.plane, lake.minimum, lake.maximum);
    }
}
