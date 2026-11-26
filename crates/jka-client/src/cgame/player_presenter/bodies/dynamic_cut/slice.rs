//! Plane split of skinned Ghoul2 surfaces into labelled pieces, plus caps.
//!
//! Port of LimbHacker (Noble Muffins, MIT; fork of Turbo Slicer):
//! `ThreadSafeHack.Slice` (per-triangle split actions, shared-edge cut
//! vertices, bone weights carried by the side's own endpoint) and
//! `Triangulation` (John Ratcliff's ear clipper, with LimbHacker's "sloppy"
//! centroid fan as the fallback infill).
//!
//! Generalized for free saber cuts:
//! - the caller labels every vertex with its piece (a skeleton-graph decision,
//!   LimbHacker's severed/severable/mandatory classification extended to many
//!   crossed bones), so one plane can separate several pieces at once;
//! - Ghoul2 bodies are many open surfaces, so cut points are welded by posed
//!   position across surfaces before cap loops are walked;
//! - every closed (or nearly closed) loop is ear clipped on its own.
//!
//! All geometry stays in bind space with bone weights, so cut pieces skin
//! through the stock CPU and GPU Ghoul2 paths under any later ragdoll pose.

use jka_assets::ghoul2::{GlmSurface, GlmVertex, GlmWeight, Matrix3x4};
use std::collections::HashMap;

pub(super) type Label = u8;

/// One source surface with its posed (model-space) vertex positions and the
/// piece label of every vertex.
pub(super) struct SliceSource<'a> {
    pub surface: &'a GlmSurface,
    pub posed: Vec<[f32; 3]>,
    pub labels: Vec<Label>,
}

/// Posed model-space plane `dot(normal, p) + distance = 0`, unit normal.
#[derive(Clone, Copy, Debug)]
pub(super) struct Plane {
    pub normal: [f32; 3],
    pub distance: f32,
}

impl Plane {
    fn side(&self, p: [f32; 3]) -> f32 {
        dot(self.normal, p) + self.distance
    }
}

/// Where a cap was generated, for blood/smoke FX (posed model space).
#[derive(Clone, Copy, Debug)]
pub(super) struct CutFace {
    pub label: Label,
    pub centroid: [f32; 3],
    /// Outward normal of this piece's cap.
    pub normal: [f32; 3],
}

pub(super) struct SliceOutput {
    /// Per label: the cut copy of every source surface that kept triangles.
    pub pieces: Vec<Vec<GlmSurface>>,
    /// Per label: generated cap triangles (surface_index set by the caller).
    pub caps: Vec<Option<GlmSurface>>,
    pub faces: Vec<CutFace>,
}

const WELD_QUANTUM: f32 = 16.0;
/// Chain ends this close are one seam (duplicated seam vertices skinned
/// slightly apart); farther ends belong to different layers.
const STITCH_REACH: f32 = 1.5;
/// Contour simplification tolerance in model units (sub-pixel at any range).
const SIMPLIFY_TOLERANCE: f32 = 0.02;

#[derive(Clone)]
struct SideVertex {
    bind: [f32; 3],
    /// (GLA bone, weight), already resolved through the source surface.
    weights: Vec<(usize, f32)>,
}

#[derive(Clone)]
struct CutPoint {
    posed: [f32; 3],
    positive: SideVertex,
    negative: SideVertex,
}

struct CapSegment {
    positive_label: Label,
    negative_label: Label,
    points: [CutPoint; 2],
}

#[derive(Default)]
struct SurfaceBuilder {
    vertices: Vec<GlmVertex>,
    texcoords: Vec<[f32; 2]>,
    triangles: Vec<[u32; 3]>,
    remap: HashMap<u32, u32>,
    /// (low vertex, high vertex) -> cut vertex on this builder's side.
    cut_remap: HashMap<(u32, u32), u32>,
}

impl SurfaceBuilder {
    fn original(&mut self, surface: &GlmSurface, index: u32) -> u32 {
        *self.remap.entry(index).or_insert_with(|| {
            let i = index as usize;
            self.vertices.push(surface.vertices[i].clone());
            self.texcoords.push(surface.texcoords[i]);
            (self.vertices.len() - 1) as u32
        })
    }
}

/// Vertex reference inside one clipped polygon.
#[derive(Clone, Copy)]
enum PolyVertex {
    Original(u32),
    /// Cut on edge (low, high) with parameter `t` from low to high.
    Cut { low: u32, high: u32, t: f32, side_endpoint: u32 },
}

pub(super) fn slice(
    sources: &[SliceSource<'_>],
    plane: Plane,
    label_count: usize,
    pose: &[Matrix3x4],
    cap_uv_scale: f32,
) -> SliceOutput {
    let front_sign = front_face_sign(sources);
    let mut pieces: Vec<Vec<GlmSurface>> = (0..label_count).map(|_| Vec::new()).collect();
    let mut segments = Vec::<CapSegment>::new();

    for source in sources {
        let surface = source.surface;
        let mut builders: Vec<SurfaceBuilder> =
            (0..label_count).map(|_| SurfaceBuilder::default()).collect();
        let distances: Vec<f32> = source.posed.iter().map(|&p| plane.side(p)).collect();

        for triangle in &surface.triangles {
            let labels = triangle.map(|i| source.labels[i as usize]);
            if labels[0] == labels[1] && labels[1] == labels[2] {
                push_original(&mut builders[usize::from(labels[0])], surface, *triangle);
                continue;
            }
            let s = triangle.map(|i| distances[i as usize]);
            let positive = s.map(|value| value >= 0.0);
            let straddles = positive.iter().any(|&p| p) && positive.iter().any(|&p| !p);
            if !straddles {
                // Skeleton and plane disagree here (e.g. a cut grazing a joint).
                // LimbHacker assigns such triangles wholesale; take the majority.
                let label = majority_label(&labels, &s, |_| true);
                push_original(&mut builders[usize::from(label)], surface, *triangle);
                continue;
            }
            let positive_label = majority_label(&labels, &s, |k| positive[k]);
            let negative_label = majority_label(&labels, &s, |k| !positive[k]);

            // Split action pass: walk A->B around the triangle exactly as
            // ThreadSafeHack does, emitting B and any A/B intersection.
            let mut positive_poly = Vec::<PolyVertex>::with_capacity(4);
            let mut negative_poly = Vec::<PolyVertex>::with_capacity(4);
            let mut cuts = Vec::<CutPoint>::with_capacity(2);
            for b in 0..3 {
                let a = (b + 2) % 3;
                let (ia, ib) = (triangle[a], triangle[b]);
                if positive[a] != positive[b] {
                    let (low, high) = canonical_edge(&source.posed, ia, ib);
                    let (sl, sh) = (distances[low as usize], distances[high as usize]);
                    let t = (sl / (sl - sh)).clamp(0.0, 1.0);
                    let positive_end = if positive[a] { ia } else { ib };
                    let negative_end = if positive[a] { ib } else { ia };
                    positive_poly.push(PolyVertex::Cut { low, high, t, side_endpoint: positive_end });
                    negative_poly.push(PolyVertex::Cut { low, high, t, side_endpoint: negative_end });
                    if positive_label != negative_label {
                        let posed = lerp3(source.posed[low as usize], source.posed[high as usize], t);
                        cuts.push(CutPoint {
                            posed,
                            positive: side_vertex(surface, low, high, t, positive_end),
                            negative: side_vertex(surface, low, high, t, negative_end),
                        });
                    }
                }
                if positive[b] {
                    positive_poly.push(PolyVertex::Original(ib));
                } else {
                    negative_poly.push(PolyVertex::Original(ib));
                }
            }
            emit_polygon(&mut builders[usize::from(positive_label)], surface, &positive_poly);
            emit_polygon(&mut builders[usize::from(negative_label)], surface, &negative_poly);
            if cuts.len() == 2 {
                let second = cuts.pop().expect("two cut points");
                let first = cuts.pop().expect("two cut points");
                segments.push(CapSegment { positive_label, negative_label, points: [first, second] });
            }
        }

        for (label, builder) in builders.into_iter().enumerate() {
            if builder.triangles.is_empty() {
                continue;
            }
            pieces[label].push(GlmSurface {
                surface_index: surface.surface_index,
                vertices: builder.vertices,
                texcoords: builder.texcoords,
                triangles: builder.triangles,
                bone_references: surface.bone_references.clone(),
            });
        }
    }

    let (caps, faces) = build_caps(&segments, plane, label_count, pose, cap_uv_scale, front_sign);
    SliceOutput { pieces, caps, faces }
}

fn push_original(builder: &mut SurfaceBuilder, surface: &GlmSurface, triangle: [u32; 3]) {
    let mapped = triangle.map(|index| builder.original(surface, index));
    builder.triangles.push(mapped);
}

/// Majority label among the selected triangle corners; ties go to the corner
/// farthest from the plane, which is the least ambiguous classification.
fn majority_label(labels: &[Label; 3], s: &[f32; 3], select: impl Fn(usize) -> bool) -> Label {
    let mut best = None::<(usize, f32, Label)>;
    for k in (0..3).filter(|&k| select(k)) {
        let count = (0..3).filter(|&j| select(j) && labels[j] == labels[k]).count();
        let key = (count, s[k].abs());
        if best.is_none_or(|(c, d, _)| key.0 > c || (key.0 == c && key.1 > d)) {
            best = Some((key.0, key.1, labels[k]));
        }
    }
    best.map_or(labels[0], |(_, _, label)| label)
}

/// Order edge endpoints by posed position so a seam shared by two surfaces
/// (duplicated vertices, identical positions) yields bit-identical cut points.
fn canonical_edge(posed: &[[f32; 3]], a: u32, b: u32) -> (u32, u32) {
    let (pa, pb) = (posed[a as usize], posed[b as usize]);
    let less = pa[0] < pb[0]
        || (pa[0] == pb[0] && (pa[1] < pb[1] || (pa[1] == pb[1] && pa[2] <= pb[2])));
    if less { (a, b) } else { (b, a) }
}

fn side_vertex(surface: &GlmSurface, low: u32, high: u32, t: f32, endpoint: u32) -> SideVertex {
    let (vl, vh) = (&surface.vertices[low as usize], &surface.vertices[high as usize]);
    let weights = surface.vertices[endpoint as usize]
        .weights
        .iter()
        .filter_map(|w| Some((*surface.bone_references.get(w.local_bone_index)?, w.weight)))
        .collect();
    SideVertex { bind: lerp3(vl.position, vh.position, t), weights }
}

fn emit_polygon(builder: &mut SurfaceBuilder, surface: &GlmSurface, poly: &[PolyVertex]) {
    if poly.len() < 3 {
        return;
    }
    let indices: Vec<u32> = poly
        .iter()
        .map(|vertex| match *vertex {
            PolyVertex::Original(index) => builder.original(surface, index),
            PolyVertex::Cut { low, high, t, side_endpoint } => {
                *builder.cut_remap.entry((low, high)).or_insert_with(|| {
                    let (vl, vh) = (&surface.vertices[low as usize], &surface.vertices[high as usize]);
                    // LimbHacker copies the side's own endpoint weights instead
                    // of blending across the cut, so each piece skins only
                    // against bones on its side of the sever.
                    builder.vertices.push(GlmVertex {
                        normal: normalize(lerp3(vl.normal, vh.normal, t)),
                        position: lerp3(vl.position, vh.position, t),
                        weights: surface.vertices[side_endpoint as usize].weights.clone(),
                    });
                    let (ul, uh) = (surface.texcoords[low as usize], surface.texcoords[high as usize]);
                    builder.texcoords.push([ul[0] + (uh[0] - ul[0]) * t, ul[1] + (uh[1] - ul[1]) * t]);
                    (builder.vertices.len() - 1) as u32
                })
            }
        })
        .collect();
    // ThreadSafeHack DirectTransferStage: triangle stays a triangle; a quad
    // is emitted as the fan (0,1,2),(0,2,3), preserving the source winding.
    for k in 1..indices.len() - 1 {
        builder.triangles.push([indices[0], indices[k], indices[k + 1]]);
    }
}

/// +1 when GLM front faces satisfy dot(cross(b-a, c-a), vertex normal) > 0.
fn front_face_sign(sources: &[SliceSource<'_>]) -> f32 {
    let mut sum = 0.0f32;
    for source in sources {
        let surface = source.surface;
        for triangle in surface.triangles.iter().take(64) {
            let [a, b, c] = triangle.map(|i| &surface.vertices[i as usize]);
            let face = cross(sub(b.position, a.position), sub(c.position, a.position));
            sum += dot(face, add(add(a.normal, b.normal), c.normal));
        }
    }
    if sum < 0.0 { -1.0 } else { 1.0 }
}

#[derive(Default)]
struct CapBuilder {
    vertices: Vec<GlmVertex>,
    texcoords: Vec<[f32; 2]>,
    triangles: Vec<[u32; 3]>,
    bone_references: Vec<usize>,
    bone_map: HashMap<usize, usize>,
}

impl CapBuilder {
    fn vertex(&mut self, side: &SideVertex, normal_bind: [f32; 3], uv: [f32; 2]) -> u32 {
        let mut weights = Vec::with_capacity(side.weights.len().min(4));
        for &(bone, weight) in side.weights.iter().take(4) {
            let local = *self.bone_map.entry(bone).or_insert_with(|| {
                self.bone_references.push(bone);
                self.bone_references.len() - 1
            });
            weights.push(GlmWeight { local_bone_index: local, weight });
        }
        if weights.is_empty() {
            let local = *self.bone_map.entry(0).or_insert_with(|| {
                self.bone_references.push(0);
                self.bone_references.len() - 1
            });
            weights.push(GlmWeight { local_bone_index: local, weight: 1.0 });
        }
        self.vertices.push(GlmVertex { normal: normal_bind, position: side.bind, weights });
        self.texcoords.push(uv);
        (self.vertices.len() - 1) as u32
    }
}

fn build_caps(
    segments: &[CapSegment],
    plane: Plane,
    label_count: usize,
    pose: &[Matrix3x4],
    cap_uv_scale: f32,
    front_sign: f32,
) -> (Vec<Option<GlmSurface>>, Vec<CutFace>) {
    let mut builders: Vec<CapBuilder> = (0..label_count).map(|_| CapBuilder::default()).collect();
    let mut faces = Vec::new();
    let (u_axis, v_axis) = plane_basis(plane.normal);

    let mut pairs: Vec<(Label, Label)> = segments
        .iter()
        .map(|s| (s.positive_label, s.negative_label))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();

    for (positive_label, negative_label) in pairs {
        // Weld cut points shared by adjacent triangles / adjacent surfaces.
        let mut ids = HashMap::<[i32; 3], usize>::new();
        let mut points = Vec::<CutPoint>::new();
        let mut edges = Vec::<(usize, usize)>::new();
        for segment in segments
            .iter()
            .filter(|s| s.positive_label == positive_label && s.negative_label == negative_label)
        {
            let mut id = |point: &CutPoint| {
                let key = point.posed.map(|v| (v * WELD_QUANTUM).round() as i32);
                *ids.entry(key).or_insert_with(|| {
                    points.push(point.clone());
                    points.len() - 1
                })
            };
            let a = id(&segment.points[0]);
            let b = id(&segment.points[1]);
            if a != b {
                edges.push((a, b));
            }
        }
        for loop_ids in walk_loops(&points, &edges) {
            let contour: Vec<[f32; 2]> = loop_ids
                .iter()
                .map(|&id| [dot(points[id].posed, u_axis), dot(points[id].posed, v_axis)])
                .collect();
            let centroid = loop_ids
                .iter()
                .fold([0.0; 3], |acc, &id| add(acc, points[id].posed))
                .map(|v| v / loop_ids.len() as f32);
            for (label, outward, positive_side) in [
                (positive_label, scale(plane.normal, -1.0), true),
                (negative_label, plane.normal, false),
            ] {
                let builder = &mut builders[usize::from(label)];
                let base = builder.vertices.len() as u32;
                for (&id, uv) in loop_ids.iter().zip(&contour) {
                    let side = if positive_side { &points[id].positive } else { &points[id].negative };
                    let normal_bind = bind_normal(pose, side, outward);
                    builder.vertex(side, normal_bind, [uv[0] / cap_uv_scale, uv[1] / cap_uv_scale]);
                }
                // (u, v, n) is right handed, so a positive 2D area means the
                // posed face normal is +n. Front faces follow the GLM winding.
                let wanted = dot(plane.normal, outward) * front_sign;
                let mut triangles = Vec::new();
                if triangulate(&contour, &mut triangles) {
                    for t in triangles.chunks_exact(3) {
                        let area = area2(contour[t[0] as usize], contour[t[1] as usize], contour[t[2] as usize]);
                        push_cap_triangle(builder, [base + t[0], base + t[1], base + t[2]], area * wanted);
                    }
                } else {
                    // LimbHacker "sloppy" infill: centroid fan, centroid bound
                    // to the loop's dominant bone.
                    let first = if positive_side { &points[loop_ids[0]].positive } else { &points[loop_ids[0]].negative };
                    let center = centroid_vertex(pose, first, centroid);
                    let normal_bind = bind_normal(pose, &center, outward);
                    let hub2 = [dot(centroid, u_axis), dot(centroid, v_axis)];
                    let hub = builder.vertex(&center, normal_bind, [hub2[0] / cap_uv_scale, hub2[1] / cap_uv_scale]);
                    let n = loop_ids.len();
                    for k in 0..n {
                        let area = area2(hub2, contour[k], contour[(k + 1) % n]);
                        push_cap_triangle(builder, [hub, base + k as u32, base + ((k + 1) % n) as u32], area * wanted);
                    }
                }
                faces.push(CutFace { label, centroid, normal: outward });
            }
        }
    }

    let caps = builders
        .into_iter()
        .map(|b| {
            (!b.triangles.is_empty()).then(|| GlmSurface {
                surface_index: usize::MAX,
                vertices: b.vertices,
                texcoords: b.texcoords,
                triangles: b.triangles,
                bone_references: b.bone_references,
            })
        })
        .collect();
    (caps, faces)
}

/// `orientation` > 0 keeps the winding, < 0 flips it; degenerate ears drop.
fn push_cap_triangle(builder: &mut CapBuilder, mut t: [u32; 3], orientation: f32) {
    if orientation == 0.0 {
        return;
    }
    if orientation < 0.0 {
        t.swap(1, 2);
    }
    builder.triangles.push(t);
}

fn area2(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Undirected loop walk over welded cut edges, then chain stitching.
///
/// A Ghoul2 cross-section is rarely one closed loop: every surface (skin,
/// sleeve, glove, belt) contributes its own open chain, and duplicated seam
/// vertices skin to slightly different positions, so chains end near each
/// other without sharing a welded point. Open chains are joined across seam
/// gaps only; every remaining chain is closed and filled on its own: a
/// slightly imperfect cap is far less visible than a see-through hole.
fn walk_loops(points: &[CutPoint], edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let (closed, open) = walk_chains(edges);
    let mut loops = Vec::with_capacity(closed.len() + open.len());
    for chain in closed.into_iter().chain(stitch_chains(points, open)) {
        // FindRealPolygon-style cleanup: drop collinear interior points so
        // the ear clipper sees a cleaner contour.
        let cleaned = drop_collinear(points, chain);
        if cleaned.len() >= 3 {
            loops.push(cleaned);
        }
    }
    loops
}

/// Greedy nearest-endpoint joining of open chains, limited to real seam
/// gaps (`STITCH_REACH`, absolute). A larger, section-relative reach joined
/// an outer layer's chain (clothing, armour) to the inner layer's chain
/// under it, which ear clips into a ring with a see-through middle. Chains
/// that stay apart are each closed and filled solid instead; overlapping
/// coplanar caps share planar UVs and texture, so the overlap is invisible.
fn stitch_chains(points: &[CutPoint], mut open: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
    let reach = STITCH_REACH;
    loop {
        let mut best = None::<(f32, usize, usize, bool, bool)>;
        for a in 0..open.len() {
            for b in 0..open.len() {
                if a == b {
                    continue;
                }
                // Try tail(a)->head(b) with either chain reversed.
                for (rev_a, rev_b) in [(false, false), (false, true), (true, false), (true, true)] {
                    let tail = if rev_a { open[a][0] } else { *open[a].last().unwrap_or(&open[a][0]) };
                    let head = if rev_b { *open[b].last().unwrap_or(&open[b][0]) } else { open[b][0] };
                    let d = length(sub(points[tail].posed, points[head].posed));
                    if d <= reach && best.is_none_or(|(bd, ..)| d < bd) {
                        best = Some((d, a, b, rev_a, rev_b));
                    }
                }
            }
        }
        let Some((_, a, b, rev_a, rev_b)) = best else { break };
        let mut chain_b = open.swap_remove(b);
        let a = if a == open.len() { b } else { a };
        if rev_b {
            chain_b.reverse();
        }
        if rev_a {
            open[a].reverse();
        }
        open[a].extend(chain_b);
    }
    open
}

/// Closed loops and open chains of welded cut points.
fn walk_chains(edges: &[(usize, usize)]) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for (index, &(a, b)) in edges.iter().enumerate() {
        adjacency.entry(a).or_default().push(index);
        adjacency.entry(b).or_default().push(index);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();
    let mut open = Vec::new();
    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (first, mut current) = edges[start];
        let mut chain = vec![first, current];
        // Extend forward, then backward from the first point.
        loop {
            let next = adjacency.get(&current).and_then(|list| {
                list.iter().copied().find(|&e| !used[e])
            });
            let Some(edge) = next else { break };
            used[edge] = true;
            let (a, b) = edges[edge];
            current = if a == current { b } else { a };
            if current == first {
                break;
            }
            chain.push(current);
        }
        if current != first {
            let mut head = first;
            loop {
                let next = adjacency.get(&head).and_then(|list| {
                    list.iter().copied().find(|&e| !used[e])
                });
                let Some(edge) = next else { break };
                used[edge] = true;
                let (a, b) = edges[edge];
                head = if a == head { b } else { a };
                if head == *chain.last().unwrap_or(&head) {
                    break;
                }
                chain.insert(0, head);
            }
            open.push(chain);
            continue;
        }
        loops.push(chain);
    }
    (loops, open)
}

/// FindRealPolygon-style cleanup with a sub-pixel tolerance: points whose
/// removal moves the contour less than `SIMPLIFY_TOLERANCE` (and duplicates)
/// are dropped, which shortens dense loops before ear clipping without
/// opening visible cracks between the cap and the cut wall.
fn drop_collinear(points: &[CutPoint], chain: Vec<usize>) -> Vec<usize> {
    if chain.len() <= 3 {
        return chain;
    }
    let n = chain.len();
    let mut out: Vec<usize> = Vec::with_capacity(n);
    for k in 0..n {
        let prev = out.last().map_or(points[chain[n - 1]].posed, |&id| points[id].posed);
        let here = points[chain[k]].posed;
        let next = points[chain[(k + 1) % n]].posed;
        let span = sub(next, prev);
        let span_len = length(span);
        let deviation = if span_len > 1.0e-6 {
            length(cross(span, sub(here, prev))) / span_len
        } else {
            length(sub(here, prev))
        };
        if deviation > SIMPLIFY_TOLERANCE {
            out.push(chain[k]);
        }
    }
    if out.len() >= 3 { out } else { chain }
}

/// John Ratcliff's ear clipper as used by LimbHacker's meticulous infill.
/// Same ear order and output; only reflex corners are tested for
/// containment (a convex corner can never lie inside a candidate ear),
/// which keeps long cut loops on high-poly models cheap.
fn triangulate(contour: &[[f32; 2]], result: &mut Vec<u32>) -> bool {
    let n = contour.len();
    if n < 3 {
        return false;
    }
    let area: f32 = (0..n)
        .map(|q| {
            let p = (q + n - 1) % n;
            contour[p][0] * contour[q][1] - contour[q][0] * contour[p][1]
        })
        .sum::<f32>()
        * 0.5;
    let mut v: Vec<usize> = if area > 0.0 { (0..n).collect() } else { (0..n).rev().collect() };
    let is_reflex = |v: &[usize], k: usize| {
        let m = v.len();
        area2(contour[v[(k + m - 1) % m]], contour[v[k]], contour[v[(k + 1) % m]]) <= 0.0
    };
    let mut reflex: Vec<bool> = (0..n).map(|k| is_reflex(&v, k)).collect();
    let mut nv = n;
    let mut count = 2 * nv;
    let mut vi = nv - 1;
    while nv > 2 {
        if count == 0 {
            result.clear();
            return false;
        }
        count -= 1;
        let mut u = vi;
        if nv <= u {
            u = 0;
        }
        vi = u + 1;
        if nv <= vi {
            vi = 0;
        }
        let mut w = vi + 1;
        if nv <= w {
            w = 0;
        }
        if snip(contour, u, vi, w, nv, &v, &reflex) {
            result.extend_from_slice(&[v[u] as u32, v[vi] as u32, v[w] as u32]);
            v.remove(vi);
            reflex.remove(vi);
            nv -= 1;
            count = 2 * nv;
            if nv >= 3 {
                for neighbour in [u, w] {
                    let k = if neighbour > vi { neighbour - 1 } else { neighbour };
                    reflex[k] = is_reflex(&v, k);
                }
            }
        }
    }
    true
}

fn snip(
    contour: &[[f32; 2]],
    u: usize,
    v: usize,
    w: usize,
    n: usize,
    indices: &[usize],
    reflex: &[bool],
) -> bool {
    let [ax, ay] = contour[indices[u]];
    let [bx, by] = contour[indices[v]];
    let [cx, cy] = contour[indices[w]];
    if f32::EPSILON > ((bx - ax) * (cy - ay)) - ((by - ay) * (cx - ax)) {
        return false;
    }
    for p in 0..n {
        if p == u || p == v || p == w || !reflex[p] {
            continue;
        }
        let [px, py] = contour[indices[p]];
        if inside_triangle([ax, ay], [bx, by], [cx, cy], [px, py]) {
            return false;
        }
    }
    true
}

fn inside_triangle(a: [f32; 2], b: [f32; 2], c: [f32; 2], p: [f32; 2]) -> bool {
    let (ax, ay) = (c[0] - b[0], c[1] - b[1]);
    let (bx, by) = (a[0] - c[0], a[1] - c[1]);
    let (cx, cy) = (b[0] - a[0], b[1] - a[1]);
    let (apx, apy) = (p[0] - a[0], p[1] - a[1]);
    let (bpx, bpy) = (p[0] - b[0], p[1] - b[1]);
    let (cpx, cpy) = (p[0] - c[0], p[1] - c[1]);
    let a_cross_bp = ax * bpy - ay * bpx;
    let c_cross_ap = cx * apy - cy * apx;
    let b_cross_cp = bx * cpy - by * cpx;
    a_cross_bp >= 0.0 && b_cross_cp >= 0.0 && c_cross_ap >= 0.0
}

fn dominant_bone(side: &SideVertex) -> usize {
    side.weights
        .iter()
        .copied()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(bone, _)| bone)
}

/// Cap normals are authored in posed space; bring them back through the
/// dominant bone so skinning (which rotates normals by weight 0's bone, as
/// OpenJK's fast-normal path does) reproduces the outward face direction.
fn bind_normal(pose: &[Matrix3x4], side: &SideVertex, outward: [f32; 3]) -> [f32; 3] {
    let bone = side.weights.first().map_or(dominant_bone(side), |w| w.0);
    let Some(m) = pose.get(bone) else { return outward };
    normalize([
        m[0][0] * outward[0] + m[1][0] * outward[1] + m[2][0] * outward[2],
        m[0][1] * outward[0] + m[1][1] * outward[1] + m[2][1] * outward[2],
        m[0][2] * outward[0] + m[1][2] * outward[1] + m[2][2] * outward[2],
    ])
}

fn centroid_vertex(pose: &[Matrix3x4], reference: &SideVertex, posed: [f32; 3]) -> SideVertex {
    let bone = dominant_bone(reference);
    let bind = pose
        .get(bone)
        .and_then(inverse_affine)
        .map_or(reference.bind, |inv| transform_point(&inv, posed));
    SideVertex { bind, weights: vec![(bone, 1.0)] }
}

fn plane_basis(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let helper = if n[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let u = normalize(cross(helper, n));
    let v = cross(n, u);
    (u, v)
}

fn inverse_affine(m: &Matrix3x4) -> Option<Matrix3x4> {
    let a = glam::Mat3::from_cols(
        glam::Vec3::new(m[0][0], m[1][0], m[2][0]),
        glam::Vec3::new(m[0][1], m[1][1], m[2][1]),
        glam::Vec3::new(m[0][2], m[1][2], m[2][2]),
    );
    if a.determinant().abs() < 1.0e-8 {
        return None;
    }
    let inv = a.inverse();
    let t = inv * glam::Vec3::new(m[0][3], m[1][3], m[2][3]);
    Some([
        [inv.x_axis.x, inv.y_axis.x, inv.z_axis.x, -t.x],
        [inv.x_axis.y, inv.y_axis.y, inv.z_axis.y, -t.y],
        [inv.x_axis.z, inv.y_axis.z, inv.z_axis.z, -t.z],
    ])
}

fn transform_point(m: &Matrix3x4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
    ]
}

pub(super) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(super) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub(super) fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(super) fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(super) fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub(super) fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
pub(super) fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 1.0e-12 { scale(a, 1.0 / l) } else { [0.0, 0.0, 1.0] }
}
pub(super) fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weight() -> Vec<GlmWeight> {
        vec![GlmWeight { local_bone_index: 0, weight: 1.0 }]
    }

    /// Closed axis-aligned box (12 triangles), outward CCW winding.
    fn cube() -> GlmSurface {
        let p = [
            [-1., -1., -1.], [1., -1., -1.], [1., 1., -1.], [-1., 1., -1.],
            [-1., -1., 1.], [1., -1., 1.], [1., 1., 1.], [-1., 1., 1.],
        ];
        let tris = [
            [0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4],
            [2, 3, 7], [2, 7, 6], [1, 2, 6], [1, 6, 5], [3, 0, 4], [3, 4, 7],
        ];
        GlmSurface {
            surface_index: 3,
            vertices: p.iter().map(|&position| GlmVertex {
                normal: normalize(position), position, weights: weight(),
            }).collect(),
            texcoords: vec![[0.0, 0.0]; 8],
            triangles: tris.to_vec(),
            bone_references: vec![0],
        }
    }

    #[test]
    fn box_split_by_plane_yields_two_capped_pieces() {
        let surface = cube();
        let plane = Plane { normal: [0.0, 0.0, 1.0], distance: -0.25 };
        let posed: Vec<_> = surface.vertices.iter().map(|v| v.position).collect();
        let labels = posed.iter().map(|p| u8::from(plane.side(*p) < 0.0)).collect();
        let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];
        let out = slice(&[SliceSource { surface: &surface, posed, labels }], plane, 2, &[identity], 8.0);
        for label in 0..2 {
            assert_eq!(out.pieces[label].len(), 1);
            let cap = out.caps[label].as_ref().expect("cap");
            assert_eq!(cap.triangles.len(), 2, "square section ear clips into two triangles");
            for t in &cap.triangles {
                let [a, b, c] = t.map(|i| cap.vertices[i as usize].position);
                let face = cross(sub(b, a), sub(c, a));
                let expected = if label == 0 { -1.0 } else { 1.0 };
                assert!(face[2] * expected > 0.0, "cap of piece {label} faces outward");
            }
        }
        // Each piece stays on its own side; cut vertices land on the plane.
        assert!(out.pieces[0][0].vertices.iter().all(|v| v.position[2] >= 0.25 - 1e-4));
        assert!(out.pieces[1][0].vertices.iter().all(|v| v.position[2] <= 0.25 + 1e-4));
    }

    #[test]
    fn ear_clipper_handles_concave_contour() {
        let l = [[0., 0.], [2., 0.], [2., 1.], [1., 1.], [1., 2.], [0., 2.]];
        let mut out = Vec::new();
        assert!(triangulate(&l, &mut out));
        assert_eq!(out.len(), 12);
    }
}
