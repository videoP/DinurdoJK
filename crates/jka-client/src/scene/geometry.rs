//! Geometry.
use crate::scene::{jka_position, Arc, BTreeMap, DrawBatch, DrawClass, GpuVertex, Vec3};

#[derive(Default)]
pub(in crate::scene) struct Geometry {
    /// Exact PVS pieces inside one coarse material/lightmap batch, keyed by
    /// camera-cluster visibility and area membership. Pieces are laid out
    /// contiguously so FULL and AUTO 4 reference subranges of the coarse range
    /// without duplicating world vertices.
    pub(in crate::scene) by_pvs_signature: BTreeMap<(Vec<u64>, [u64; 4]), WorldGeometryChunk>,
}

#[derive(Default)]
pub(in crate::scene) struct WorldGeometryChunk {
    pub(in crate::scene) patches: Vec<super::PreparedPatchDraw>,
    pub(in crate::scene) vertices: Vec<GpuVertex>,
    pub(in crate::scene) surface_ids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::scene) struct GrassPatchKey {
    pub(in crate::scene) cell_x: i32,
    pub(in crate::scene) cell_z: i32,
    pub(in crate::scene) pvs_signature: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::scene) struct GrassWorkerPatchKey {
    pub(in crate::scene) cell_x: i32,
    pub(in crate::scene) cell_z: i32,
    pub(in crate::scene) signature_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::scene) struct GroupKey {
    /// TaystJK orders whole shaders by authored sort then sorted shader index.
    /// Sky normally sorts at 2, opaque at 3, explicit late sorts afterward.
    /// Stage pipeline class is NOT a primary ordering dimension.
    pub(in crate::scene) sort_priority: i32,
    pub(in crate::scene) shader: usize,
    pub(in crate::scene) class: DrawClass,
    /// Keep distinct lightstyle geometry independent even when pages alias.
    pub(in crate::scene) lightstyle_slot: u8,
    pub(in crate::scene) lightmap: Option<usize>,
    pub(in crate::scene) vertex_lit: bool,
    pub(in crate::scene) color_slot: u8,
    pub(in crate::scene) fog_num: i32,
    pub(in crate::scene) transparent_order: usize,
    pub(in crate::scene) planar_group: Option<PlanarGroupKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::scene) struct PlanarGroupKey {
    pub(in crate::scene) normal_x: i32,
    pub(in crate::scene) normal_y: i32,
    pub(in crate::scene) normal_z: i32,
    pub(in crate::scene) distance: i32,
}

/// Markable-surface index for a source-.map world, which has no BSP surface
/// table to read shader flags from: every opaque/mask render triangle that faces
/// up enough is treated like an `SF_FACE`, as the old renderer-side gather did.
pub(in crate::scene) fn mark_surfaces_from_batches(
    vertices: &[GpuVertex],
    batches: &[DrawBatch],
) -> Arc<jka_assets::bsp::MarkSurfaces> {
    let mut triangles = Vec::new();
    for batch in batches {
        if !matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask) {
            continue;
        }
        let start = batch.vertices.start as usize;
        let end = (batch.vertices.end as usize).min(vertices.len());
        if start >= end {
            continue;
        }
        for triangle in vertices[start..end].chunks_exact(3) {
            let normal = Vec3::from_array(triangle[0].normal)
                + Vec3::from_array(triangle[1].normal)
                + Vec3::from_array(triangle[2].normal);
            let normal = normal.normalize_or_zero();
            if normal == Vec3::ZERO {
                continue;
            }
            triangles.push((
                [
                    triangle[0].position,
                    triangle[1].position,
                    triangle[2].position,
                ]
                .map(jka_position),
                jka_position(normal.to_array()),
            ));
        }
    }
    Arc::new(jka_assets::bsp::MarkSurfaces::from_world_triangles(
        triangles,
    ))
}


/// Pack an independent pair of BSP triangles representing a rectangular
/// autoSprite2 quad. TaystJK's Autosprite2Deform finds its two shortest,
/// opposite edges, leaves their midpoints fixed and turns the short edges
/// around the shared long axis using cross(major, view_forward).
///
/// The BSP world already uses a non-indexed vertex stream. Encoding each
/// vertex with its short-edge midpoint, shared long axis, and signed half
/// width costs no extra world vertex bytes or per-frame CPU geometry updates.
/// Without a lightmap the original compact encoding uses spare lightmap UVs;
/// with a lightmap an octahedral axis/width encoding preserves authored UVs.
///
/// If the pair is not an independent quad, leave it unchanged: the dedicated
/// vertex shader checks the encoding marker before interpreting the payload.
pub(in crate::scene) fn encode_autosprite2_quad(vertices: &mut [GpuVertex], lightmapped: bool) -> bool {
    if vertices.len() != 6 { return false; }
    // Quake 3 draws two indexed triangles using four shared positions.
    let mut points = [[0.0_f32; 3]; 4];
    let mut which = [0_usize; 6];
    let mut count = 0;
    for (i, vertex) in vertices.iter().enumerate() {
        let p = vertex.position;
        let n = (0..count).find(|&n| points[n] == p);
        which[i] = if let Some(n) = n { n } else {
            if count == 4 { return false; }
            points[count] = p;
            count += 1;
            count - 1
        };
    }
    if count != 4 || points.iter().flatten().any(|v| !v.is_finite()) { return false; }
    let edges = [(0,1), (0,2), (0,3), (1,2), (1,3), (2,3)];
    let dist2 = |a: usize, b: usize| -> f32 {
        (0..3).map(|k| (points[a][k] - points[b][k]).powi(2)).sum()
    };
    let mut candidates = edges.map(|(a,b)| (dist2(a,b), a, b));
    candidates.sort_by(|l,r| l.0.total_cmp(&r.0));
    let (_, a, b) = candidates[0];
    // The two narrow edges must be opposite (unlike a triangle fan or a patch).
    let Some(&(_, c, d)) = candidates.iter().find(|(_,c,d)|
        *c != a && *c != b && *d != a && *d != b
    ) else { return false; };
    let width2 = dist2(a,b);
    let width2_other = dist2(c,d);
    if !width2.is_finite() || !width2_other.is_finite()
        || width2 < 1.0e-8 || width2_other < 1.0e-8 { return false; }
    let mids: [[f32; 3]; 2] = [
        std::array::from_fn(|k| (points[a][k] + points[b][k]) * 0.5),
        std::array::from_fn(|k| (points[c][k] + points[d][k]) * 0.5),
    ];
    let major: [f32;3] = std::array::from_fn(|k| mids[1][k] - mids[0][k]);
    let axis_len2: f32 = major.iter().map(|v| v*v).sum();
    if axis_len2 <= width2.min(width2_other) * 0.25 { return false; }
    // This is a quad, not two arbitrarily paired triangles. TaystJK expects
    // the two opposite shortest edges of a rectangular billboard.
    if (width2 - width2_other).abs() > width2.max(width2_other) * 0.15 {
        return false;
    }
    let minor: [f32; 3] = std::array::from_fn(|k| points[b][k] - points[a][k]);
    let axis_dot_minor: f32 = (0..3).map(|k| major[k] * minor[k]).sum();
    if axis_dot_minor.abs() > (axis_len2 * width2).sqrt() * 0.15 {
        return false;
    }
    let axis_len = axis_len2.sqrt();
    let major = major.map(|v| v / axis_len);
    // The minor-edge orientation follows the two triangles' winding, matching
    // TaystJK's lookup into tess.indexes when it decides +/- half the width.
    let oriented = |a: usize, b: usize| -> bool {
        which.chunks_exact(3).any(|t| (0..3).any(|i|
            t[i] == a && t[(i+1)%3] == b
        ))
    };
    let sign_ab = if oriented(a,b) { -1.0 } else { 1.0 };
    // The second narrow edge runs in the opposite winding direction on a
    // normal quad. Preserve the SAME physical side of the billboard at both
    // ends of the long axis; applying tess-index winding separately to both
    // edges can invert the top, folding the two triangles over each other.
    let sign_cd = if dist2(a,c) <= dist2(a,d) { sign_ab } else { -sign_ab };
    let mut midpoint = [[0.0_f32;3];4];
    let mut offset = [0.0_f32;4];
    for (endpoints, center, halfwidth, sign) in [
        ((a,b), mids[0], width2.sqrt() * 0.5, sign_ab),
        ((c,d), mids[1], width2_other.sqrt() * 0.5, sign_cd),
    ] {
        midpoint[endpoints.0] = center;
        midpoint[endpoints.1] = center;
        offset[endpoints.0] = halfwidth * sign;
        offset[endpoints.1] = -halfwidth * sign;
    }
    // Non-lightmapped quads retain the existing zero-overhead format.
    // Lightmapped quads cannot overwrite the UVs: octahedrally pack the long
    // axis in normal.xy and the signed half-width into normal.z with a tag.
    // This only changes the dedicated billboard vertex shader, not GpuVertex.
    if lightmapped && offset.iter().any(|w| w.abs() >= 2048.0) {
        return false;
    }
    let packed_axis = if lightmapped { Some(oct_encode(major)) } else { None };
    for (i, vertex) in vertices.iter_mut().enumerate() {
        let index = which[i];
        vertex.position = midpoint[index];
        if let Some(axis) = packed_axis {
            vertex.normal = [axis[0], axis[1], 8192.0 + offset[index]];
        } else {
            vertex.normal = major;
            vertex.lightmap_uv[0] = offset[index];
            vertex.lightmap_uv[1] = -1337.0; // packed autoSprite2 marker
        }
    }
    true
}

// Only used for lightmapped autoSprite2 quads. Standard normal->oct projection;
// the WGPU billboard vertex entry reverses this at draw time.
fn oct_encode(axis: [f32; 3]) -> [f32; 2] {
    let div = axis.iter().map(|n| n.abs()).sum::<f32>();
    let n = axis.map(|v| v / div);
    if n[2] >= 0.0 {
        [n[0], n[1]]
    } else {
        [(1.0 - n[1].abs()) * n[0].signum(),
         (1.0 - n[0].abs()) * n[1].signum()]
    }
}

/// Port of TaystJK's AutospriteDeform (rd-vulkan/tr_shade_calc.cpp): average
/// the four quad corners, then use the first corner distance times 0.707 for
/// both billboard axes. The WGPU vertex entry supplies the current camera's
/// right/up vectors instead of rebuilding a CPU tessellation every frame.
///
/// Preserve each authored UV; position stores the center and normal.xyz is
/// two signed offsets plus a -1337 tag. Lightmap UVs remain untouched.
pub(in crate::scene) fn encode_autosprite_quad(vertices: &mut [GpuVertex]) -> bool {
    if vertices.len() != 6 { return false; }
    let mut points = [[0.0_f32; 3]; 4];
    let mut uv = [[0.0_f32; 2]; 4];
    let mut which = [0_usize; 6];
    let mut count = 0;
    for (i, vertex) in vertices.iter().enumerate() {
        if !vertex.position.iter().chain(vertex.uv.iter()).all(|v| v.is_finite()) { return false; }
        let index = (0..count).find(|&j| points[j] == vertex.position);
        which[i] = if let Some(j) = index {
            if uv[j] != vertex.uv { return false; }
            j
        } else {
            if count == 4 { return false; }
            points[count] = vertex.position;
            uv[count] = vertex.uv;
            count += 1;
            count - 1
        };
    }
    if count != 4 { return false; }
    // The two triangles must share exactly one diagonal, not merely mention
    // four different points. The authored surface can use either winding.
    if which[..3].iter().copied().collect::<std::collections::BTreeSet<_>>().len() != 3
        || which[3..].iter().copied().collect::<std::collections::BTreeSet<_>>().len() != 3
        || which[..3].iter().filter(|v| which[3..].contains(*v)).count() != 2
    { return false; }
    let center: [f32; 3] = std::array::from_fn(|k| points.iter().map(|p| p[k]).sum::<f32>() * 0.25);
    let radius = points[0].iter().zip(center)
        .map(|(p,c)| (*p-c).powi(2)).sum::<f32>().sqrt() * 0.707;
    if !radius.is_finite() || radius < 1.0e-5 { return false; }
    // The usual JKA sprite UV set has four rectangular corners. Mapping by
    // the authored UV corners preserves texture orientation when source index
    // order is changed by BSP triangle expansion.
    let umin = uv.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
    let umax = uv.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
    let vmin = uv.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let vmax = uv.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
    if umax-umin < 1.0e-5 || vmax-vmin < 1.0e-5 { return false; }
    let mut occupied = [false; 4];
    let mut sign = [[0.0_f32;2]; 4];
    for i in 0..4 {
        let u = if (uv[i][0]-umin).abs() <= (uv[i][0]-umax).abs() { 0 } else { 1 };
        let v = if (uv[i][1]-vmin).abs() <= (uv[i][1]-vmax).abs() { 0 } else { 1 };
        if (uv[i][0] - [umin,umax][u]).abs() > (umax-umin)*0.05
            || (uv[i][1] - [vmin,vmax][v]).abs() > (vmax-vmin)*0.05
            || occupied[v*2+u] { return false; }
        occupied[v*2+u] = true;
        sign[i] = [if u == 0 { -radius } else { radius },
                   if v == 0 { radius } else { -radius }];
    }
    let color = vertices[0].color; // TaystJK RB_AddQuadStamp uses vertexColors[i].
    for (i, vertex) in vertices.iter_mut().enumerate() {
        vertex.position = center;
        vertex.normal = [sign[which[i]][0], sign[which[i]][1], -1337.0];
        vertex.color = color;
    }
    true
}

/// Match independent quads by their actual shared triangle edge rather than
/// assuming two adjacent triangles in the BSP index stream. This runs once
/// during map preparation, only for authored billboard shaders.
pub(in crate::scene) fn prepare_billboard_quads(
    vertices: &mut [GpuVertex], auto_sprite: bool, lightmapped: bool,
) -> usize {
    use std::collections::HashMap;
    type Point = [u32; 3];
    type Edge = (Point, Point);
    // -0.0 and +0.0 compare equal in the source mesh: canonicalize before
    // hashing edges so a signed zero never splits an otherwise valid quad.
    let point = |v: &GpuVertex| v.position.map(|x| if x == 0.0 { 0_u32 } else { x.to_bits() });
    let mut open = HashMap::<Edge, Vec<usize>>::new();
    let mut paired = vec![false; vertices.len() / 3];
    let mut prepared = 0;
    for triangle in 0..paired.len() {
        let tri = &vertices[triangle*3..triangle*3+3];
        let p = [point(&tri[0]), point(&tri[1]), point(&tri[2])];
        let edges = [(p[0],p[1]), (p[1],p[2]), (p[2],p[0])];
        let mut success = false;
        for (mut a, mut b) in edges {
            if a > b { std::mem::swap(&mut a, &mut b); }
            if let Some(candidates) = open.get(&(a,b)) {
                for &other in candidates.iter().rev() {
                    if paired[other] { continue; }
                    let mut quad = [vertices[other*3], vertices[other*3+1], vertices[other*3+2],
                                    vertices[triangle*3], vertices[triangle*3+1], vertices[triangle*3+2]];
                    let valid = if auto_sprite { encode_autosprite_quad(&mut quad) }
                        else { encode_autosprite2_quad(&mut quad, lightmapped) };
                    if valid {
                        vertices[other*3..other*3+3].copy_from_slice(&quad[..3]);
                        vertices[triangle*3..triangle*3+3].copy_from_slice(&quad[3..]);
                        paired[other] = true;
                        paired[triangle] = true;
                        prepared += 1;
                        success = true;
                        break;
                    }
                }
            }
            if success { break; }
        }
        if !success {
            for (mut a, mut b) in edges {
                if a > b { std::mem::swap(&mut a, &mut b); }
                open.entry((a,b)).or_default().push(triangle);
            }
        }
    }
    prepared
}


#[cfg(test)]
mod autosprite2_tests {
    use super::*;
    #[test]
    fn rectangular_quad_keeps_its_long_edge_midpoints() {
        let vertex = |position| GpuVertex {
            position, uv: [0.0,0.0], lightmap_uv: [0.0,0.0],
            normal: [0.0,0.0,1.0], color: [1.0;4], alpha_cutoff: 1.0,
        };
        // 2-wide / 12-high upright tree with two triangles and 4 shared corners.
        let mut vertices = [
            vertex([-1.0,0.0,0.0]), vertex([1.0,0.0,0.0]), vertex([1.0,12.0,0.0]),
            vertex([-1.0,0.0,0.0]), vertex([1.0,12.0,0.0]), vertex([-1.0,12.0,0.0]),
        ];
        assert!(encode_autosprite2_quad(&mut vertices, false));
        for v in &vertices {
            assert_eq!(v.position[0], 0.0);
            assert_eq!(v.normal[0], 0.0);
            assert_eq!(v.normal[1].abs(), 1.0);
            assert_eq!(v.lightmap_uv[0].abs(), 1.0);
        }
        assert_eq!(vertices[0].position, vertices[1].position);
        assert_eq!(vertices[2].position, vertices[4].position);
        assert_eq!(vertices[5].position, vertices[2].position);
        assert_eq!(vertices[0].lightmap_uv[0], -vertices[1].lightmap_uv[0]);
        assert_eq!(vertices[1].lightmap_uv[0], vertices[2].lightmap_uv[0]);
        assert_eq!(vertices[0].lightmap_uv[0], vertices[5].lightmap_uv[0]);
    }

    #[test]
    fn arbitrary_triangle_pair_is_not_reinterpreted_as_a_quad() {
        let v = |position| GpuVertex { position, uv: [0.0;2],
            lightmap_uv: [0.0;2], normal: [0.0,1.0,0.0],
            color: [1.0;4], alpha_cutoff: 1.0 };
        let mut vertices = [v([0.,0.,0.]), v([1.,0.,0.]), v([0.,1.,0.]),
            v([2.,0.,0.]), v([3.,0.,0.]), v([2.,1.,0.])];
        assert!(!encode_autosprite2_quad(&mut vertices, false));
    }
}

#[cfg(test)]
mod billboard_preparation_tests {
    use super::*;
    fn v(position: [f32;3], uv: [f32;2], lm: [f32;2]) -> GpuVertex {
        GpuVertex { position, uv, lightmap_uv: lm, normal: [0.,0.,1.],
            color: [1.;4], alpha_cutoff: 0.5 }
    }
    #[test]
    fn autosprite_uses_taystjk_center_and_radius_and_keeps_lightmap_uvs() {
        let mut quad = [
            v([-2.,0.,2.],[0.,0.],[0.2,0.3]), v([2.,0.,-2.],[1.,1.],[0.4,0.5]),
            v([2.,0.,2.],[1.,0.],[0.4,0.3]), v([-2.,0.,2.],[0.,0.],[0.2,0.3]),
            v([-2.,0.,-2.],[0.,1.],[0.2,0.5]), v([2.,0.,-2.],[1.,1.],[0.4,0.5]),
        ];
        assert!(encode_autosprite_quad(&mut quad));
        assert!(quad.iter().all(|v| v.position == [0.,0.,0.]));
        assert_eq!(quad[0].normal[0], -quad[1].normal[0]);
        assert_eq!(quad[0].normal[1], -quad[1].normal[1]);
        assert_eq!(quad[0].normal[2], -1337.);
        assert_eq!(quad[0].lightmap_uv, [0.2,0.3]);
        assert_eq!(quad[1].lightmap_uv, [0.4,0.5]);
    }
    #[test]
    fn autosprite2_lightmap_encoding_preserves_source_uv() {
        let mut quad = [
            v([-1.,0.,0.],[0.,1.],[0.2,0.3]),v([1.,0.,0.],[1.,1.],[0.4,0.3]),
            v([1.,12.,0.],[1.,0.],[0.4,0.5]),v([-1.,0.,0.],[0.,1.],[0.2,0.3]),
            v([1.,12.,0.],[1.,0.],[0.4,0.5]),v([-1.,12.,0.],[0.,0.],[0.2,0.5]),
        ];
        assert!(encode_autosprite2_quad(&mut quad, true));
        assert_eq!(quad[0].lightmap_uv, [0.2,0.3]);
        assert_eq!(quad[2].lightmap_uv, [0.4,0.5]);
        assert!(quad.iter().all(|v| v.normal[2] > 2048.));
    }
    #[test]
    fn pairs_interleaved_quads_without_crossing_card_boundaries() {
        let mut geometry = [
            v([-1.,0.,0.],[0.,1.],[-1.;2]),v([1.,0.,0.],[1.,1.],[-1.;2]),v([1.,2.,0.],[1.,0.],[-1.;2]),
            v([9.,0.,0.],[0.,1.],[-1.;2]),v([11.,0.,0.],[1.,1.],[-1.;2]),v([11.,2.,0.],[1.,0.],[-1.;2]),
            v([-1.,0.,0.],[0.,1.],[-1.;2]),v([1.,2.,0.],[1.,0.],[-1.;2]),v([-1.,2.,0.],[0.,0.],[-1.;2]),
            v([9.,0.,0.],[0.,1.],[-1.;2]),v([11.,2.,0.],[1.,0.],[-1.;2]),v([9.,2.,0.],[0.,0.],[-1.;2]),
        ];
        assert_eq!(prepare_billboard_quads(&mut geometry, true, false), 2);
        assert!(geometry[..12].iter().all(|v| v.normal[2] == -1337.));
        assert_eq!(geometry[0].position, [0.,1.,0.]);
        assert_eq!(geometry[3].position, [10.,1.,0.]);
    }
}
