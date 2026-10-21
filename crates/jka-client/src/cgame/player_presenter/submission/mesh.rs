//! Submission mesh.
use crate::cgame::jiggle::GluteShape;
use crate::cgame::player_presenter::{
    Arc, Ghoul2GpuBone, Ghoul2GpuMeshSource, Ghoul2GpuVertex, GlmModel, GlmSurface, HashMap,
    JiggleProfile, JiggleRemeshTuning, Matrix3x4, Vec3,
};

pub(in crate::cgame::player_presenter) fn gpu_vertex_from_glm(
    surface: &GlmSurface,
    vertex_index: usize,
    jiggle_profile: Option<&JiggleProfile>,
    lod_index: usize,
) -> Result<Ghoul2GpuVertex, String> {
    let vertex = surface.vertices.get(vertex_index).ok_or_else(|| {
        format!(
            "GLM surface {} missing vertex {vertex_index}",
            surface.surface_index
        )
    })?;
    let uv = *surface.texcoords.get(vertex_index).ok_or_else(|| {
        format!(
            "GLM surface {} missing texcoord {vertex_index}",
            surface.surface_index
        )
    })?;
    if vertex.weights.is_empty() || vertex.weights.len() > 4 {
        return Err(format!(
            "GLM surface {} vertex {} has unsupported weight count {}",
            surface.surface_index,
            vertex_index,
            vertex.weights.len()
        ));
    }
    let mut bone_indices = [0u32; 4];
    let mut weights = [0.0f32; 4];
    for (weight_index, weight) in vertex.weights.iter().enumerate() {
        let skeleton_bone = *surface
            .bone_references
            .get(weight.local_bone_index)
            .ok_or_else(|| {
                format!(
                    "GLM surface {} vertex {} references missing local bone {}",
                    surface.surface_index, vertex_index, weight.local_bone_index
                )
            })?;
        bone_indices[weight_index] = u32::try_from(skeleton_bone)
            .map_err(|_| format!("GLM skeleton bone index {skeleton_bone} exceeds GPU u32"))?;
        weights[weight_index] = weight.weight;
    }
    let (jiggle_region, jiggle_weight, jiggle_coord) = jiggle_profile
        .map(|profile| profile.gpu_vertex_binding(lod_index, surface.surface_index, vertex_index))
        .unwrap_or((u32::MAX, 0.0, 4.0));
    Ok(Ghoul2GpuVertex {
        position: vertex.position,
        normal: vertex.normal,
        uv,
        bone_indices,
        weights,
        weight_count: vertex.weights.len() as u32,
        jiggle_region,
        jiggle_weight,
        jiggle_coord,
    })
}

pub(in crate::cgame::player_presenter) fn append_surface_indices(
    surface: &GlmSurface,
    two_sided: bool,
) -> Result<Vec<u32>, String> {
    let mut indices =
        Vec::with_capacity(surface.triangles.len() * 3 * if two_sided { 2 } else { 1 });
    for (triangle_index, triangle) in surface.triangles.iter().enumerate() {
        if triangle
            .iter()
            .any(|&index| index as usize >= surface.vertices.len())
        {
            return Err(format!(
                "GLM surface {} triangle {} has out-of-range vertex",
                surface.surface_index, triangle_index
            ));
        }
        let mut triangle = *triangle;
        triangle.swap(1, 2);
        indices.extend_from_slice(&triangle);
    }
    if two_sided {
        let front_count = indices.len();
        for triangle in 0..front_count / 3 {
            let base = triangle * 3;
            indices.extend_from_slice(&[indices[base], indices[base + 2], indices[base + 1]]);
        }
    }
    Ok(indices)
}

pub(in crate::cgame::player_presenter) fn blend_gpu_influences(
    a: &Ghoul2GpuVertex,
    b: &Ghoul2GpuVertex,
) -> ([u32; 4], [f32; 4], u32) {
    let mut combined = HashMap::<u32, f32>::new();
    for source in [a, b] {
        for index in 0..source.weight_count.min(4) as usize {
            *combined.entry(source.bone_indices[index]).or_default() += source.weights[index] * 0.5;
        }
    }
    let mut influences = combined.into_iter().collect::<Vec<_>>();
    influences.sort_by(|left, right| right.1.total_cmp(&left.1));
    influences.truncate(4);
    let total = influences
        .iter()
        .map(|(_, weight)| *weight)
        .sum::<f32>()
        .max(0.0001);
    let mut bone_indices = [0u32; 4];
    let mut weights = [0.0f32; 4];
    for (index, (bone, weight)) in influences.iter().enumerate() {
        bone_indices[index] = *bone;
        weights[index] = *weight / total;
    }
    (bone_indices, weights, influences.len().max(1) as u32)
}

pub(in crate::cgame::player_presenter) fn promoted_midpoint(
    a: &Ghoul2GpuVertex,
    b: &Ghoul2GpuVertex,
    curve_position: bool,
) -> Ghoul2GpuVertex {
    let pa = Vec3::from_array(a.position);
    let pb = Vec3::from_array(b.position);
    let na = Vec3::from_array(a.normal).normalize_or_zero();
    let nb = Vec3::from_array(b.normal).normalize_or_zero();
    let linear = (pa + pb) * 0.5;
    let projected_a = linear - na * (linear - pa).dot(na);
    let projected_b = linear - nb * (linear - pb).dot(nb);
    let curved = if curve_position {
        linear.lerp((projected_a + projected_b) * 0.5, 0.75)
    } else {
        // Surface boundaries/UV seams must remain on the source edge so an
        // adjacent unpromoted surface cannot develop a crack.
        linear
    };
    let normal = (na + nb).normalize_or_zero();
    let (bone_indices, weights, weight_count) = blend_gpu_influences(a, b);
    let (jiggle_region, jiggle_weight, jiggle_coord) = match (a.jiggle_region, b.jiggle_region) {
        (ra, rb) if ra == rb => (
            ra,
            (a.jiggle_weight + b.jiggle_weight) * 0.5,
            (a.jiggle_coord + b.jiggle_coord) * 0.5,
        ),
        (u32::MAX, rb) => (rb, b.jiggle_weight * 0.5, b.jiggle_coord),
        (ra, u32::MAX) => (ra, a.jiggle_weight * 0.5, a.jiggle_coord),
        (ra, _) if a.jiggle_weight >= b.jiggle_weight => {
            (ra, a.jiggle_weight * 0.5, a.jiggle_coord)
        }
        (_, rb) => (rb, b.jiggle_weight * 0.5, b.jiggle_coord),
    };
    Ghoul2GpuVertex {
        position: curved.to_array(),
        normal: if normal.length_squared() > 0.0 {
            normal.to_array()
        } else {
            a.normal
        },
        uv: [(a.uv[0] + b.uv[0]) * 0.5, (a.uv[1] + b.uv[1]) * 0.5],
        bone_indices,
        weights,
        weight_count,
        jiggle_region,
        jiggle_weight,
        jiggle_coord,
    }
}

pub(in crate::cgame::player_presenter) fn jiggle_promotion_weight(
    vertex: &Ghoul2GpuVertex,
    profile: &JiggleProfile,
    tuning: JiggleRemeshTuning,
) -> f32 {
    // Size reshapes the cached surface but never expands the motion/remesh mask.
    if vertex.jiggle_region == u32::MAX || vertex.jiggle_weight <= 0.0 {
        return 0.0;
    }
    vertex.jiggle_weight
        * profile.remesh_vertical_factor(vertex.jiggle_region, vertex.position[2], tuning)
}

pub(in crate::cgame::player_presenter) fn subdivide_jiggle_region(
    source_vertices: &[Ghoul2GpuVertex],
    source_indices: &[u32],
    threshold: f32,
    profile: &JiggleProfile,
    tuning: JiggleRemeshTuning,
) -> (Vec<Ghoul2GpuVertex>, Vec<u32>) {
    subdivide_weighted(
        source_vertices,
        source_indices,
        threshold,
        |vertex| jiggle_promotion_weight(vertex, profile, tuning),
        None,
    )
}

/// Same guard-band subdivision, with an optional H selection that preserves
/// existing midpoint geometry when I adds the posterior-thigh support patch.
fn subdivide_weighted(
    source_vertices: &[Ghoul2GpuVertex],
    source_indices: &[u32],
    threshold: f32,
    promotion_weight: impl Fn(&Ghoul2GpuVertex) -> f32,
    preserve_weight: Option<(&dyn Fn(&Ghoul2GpuVertex) -> f32, f32)>,
) -> (Vec<Ghoul2GpuVertex>, Vec<u32>) {
    let triangles = source_indices
        .chunks_exact(3)
        .map(|triangle| [triangle[0], triangle[1], triangle[2]])
        .collect::<Vec<_>>();
    let selected = triangles
        .iter()
        .map(|triangle| {
            triangle.iter().any(|&index| {
                source_vertices
                    .get(index as usize)
                    .is_some_and(|vertex| promotion_weight(vertex) > threshold)
            })
        })
        .collect::<Vec<_>>();

    let edge_key = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
    let mut edge_triangles = HashMap::<(u32, u32), Vec<usize>>::new();
    for (triangle_index, triangle) in triangles.iter().enumerate() {
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            edge_triangles
                .entry(edge_key(a, b))
                .or_default()
                .push(triangle_index);
        }
    }

    // Leave one complete source-triangle band between the curved promoted core
    // and the stock mesh.  This is stronger than merely sharing a midpoint:
    // interpolated bind position + interpolated skin weights do not, in general,
    // skin to the exact midpoint of the two already-skinned source vertices.
    // Keeping the outer edge unsplit makes the promoted side and its low-res
    // neighbour use the exact same two skinned endpoints, eliminating the seam.
    let make_core = |selected: &[bool]| {
        triangles
            .iter()
            .enumerate()
            .map(|(triangle_index, triangle)| {
                selected[triangle_index]
                    && [
                        edge_key(triangle[0], triangle[1]),
                        edge_key(triangle[1], triangle[2]),
                        edge_key(triangle[2], triangle[0]),
                    ]
                    .into_iter()
                    .all(|edge| {
                        edge_triangles.get(&edge).is_some_and(|users| {
                            users.len() == 2 && users.iter().all(|&index| selected[index])
                        })
                    })
            })
            .collect::<Vec<_>>()
    };
    let core = make_core(&selected);
    let original_core = preserve_weight.map(|(weight, reference_threshold)| {
        let original_selected = triangles
            .iter()
            .map(|triangle| {
                triangle.iter().any(|&index| {
                    source_vertices
                        .get(index as usize)
                        .is_some_and(|vertex| weight(vertex) > reference_threshold)
                })
            })
            .collect::<Vec<_>>();
        make_core(&original_selected)
    });

    let mut vertices = source_vertices.to_vec();
    let mut edges = HashMap::<(u32, u32), u32>::new();
    for (&key, users) in &edge_triangles {
        // Only split an edge if it participates in the interior promoted core.
        // Edges on the outer guard-band boundary stay exactly as authored.
        if !users.iter().any(|&index| core[index]) {
            continue;
        }
        let (a, b) = key;
        let (Some(va), Some(vb)) = (
            vertices.get(a as usize).copied(),
            vertices.get(b as usize).copied(),
        ) else {
            continue;
        };
        // Already-present H edges retain their original linear/curved choice.
        // This freezes the accepted fold instead of relocating its guard ring.
        let midpoint_core = original_core
            .as_ref()
            .filter(|original| users.iter().any(|&index| original[index]))
            .unwrap_or(&core);
        let curve_position = users.len() == 2 && users.iter().all(|&index| midpoint_core[index]);
        let index = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        vertices.push(promoted_midpoint(&va, &vb, curve_position));
        edges.insert(key, index);
    }

    let edge_mid = |a: u32, b: u32| edges.get(&edge_key(a, b)).copied();
    let mut indices = Vec::with_capacity(source_indices.len() * 2);
    for triangle in &triangles {
        let a = triangle[0];
        let b = triangle[1];
        let c = triangle[2];
        let ab = edge_mid(a, b);
        let bc = edge_mid(b, c);
        let ca = edge_mid(c, a);

        // Core triangles have all three edge midpoints and become 4 triangles.
        // The one-band guard ring is split only on the edge(s) facing the core;
        // its outside edge remains the original low-res edge exactly.
        match (ab, bc, ca) {
            (None, None, None) => indices.extend_from_slice(&[a, b, c]),
            (Some(ab), None, None) => {
                indices.extend_from_slice(&[a, ab, c, ab, b, c]);
            }
            (None, Some(bc), None) => {
                indices.extend_from_slice(&[a, b, bc, a, bc, c]);
            }
            (None, None, Some(ca)) => {
                indices.extend_from_slice(&[a, b, ca, b, c, ca]);
            }
            (Some(ab), Some(bc), None) => {
                indices.extend_from_slice(&[ab, b, bc, a, ab, bc, a, bc, c]);
            }
            (None, Some(bc), Some(ca)) => {
                indices.extend_from_slice(&[bc, c, ca, b, bc, ca, b, ca, a]);
            }
            (Some(ab), None, Some(ca)) => {
                indices.extend_from_slice(&[ca, a, ab, c, ca, ab, c, ab, b]);
            }
            (Some(ab), Some(bc), Some(ca)) => {
                indices.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
            }
        }
    }
    (vertices, indices)
}

const BREAST_SECOND_PROMOTION_THRESHOLD: f32 = 0.39;
const BREAST_REFERENCE_PROMOTION_THRESHOLD: f32 = 0.18;
type PromotedGeometry = (Vec<Ghoul2GpuVertex>, Vec<u32>);

/// Optional full breast bake is temporary: only the reduced draw mesh is kept
/// in the GPU source. Existing shape/normals are sampled at its retained points.
pub(in crate::cgame::player_presenter) struct JigglePromotion {
    geometry: PromotedGeometry,
    breast_reference: Option<PromotedGeometry>,
}
impl JigglePromotion {
    fn plain(geometry: PromotedGeometry) -> Self {
        Self {
            geometry,
            breast_reference: None,
        }
    }
}

/// Two local passes: H's lower-fold/medial coverage and I's small thigh-rounding
/// patch. jedi_zf: 520 -> 1,532 hip triangles (2.95x). Source motion bindings
/// are only interpolated, never expanded or reweighted by the size slider.
/// The selected authored LOD also controls promotion: 2 / 1 / 0 passes.
pub(in crate::cgame::player_presenter) fn promote_jiggle_mesh(
    source_vertices: &[Ghoul2GpuVertex],
    source_indices: &[u32],
    profile: &JiggleProfile,
    tuning: JiggleRemeshTuning,
    lod_index: usize,
) -> JigglePromotion {
    // r_lodScale/r_lodbias choose this authored LOD upstream. Keep distant
    // meshes on their original topology; size and motion still apply below.
    if lod_index >= 2 {
        return JigglePromotion::plain((source_vertices.to_vec(), source_indices.to_vec()));
    }
    let carries = |glute| {
        source_vertices.iter().any(|vertex| {
            vertex.jiggle_weight > 0.0
                && if glute {
                    profile.is_glute_region(vertex.jiggle_region)
                } else {
                    profile.is_breast_region(vertex.jiggle_region)
                }
        })
    };
    let breast_surface = carries(false);
    let glute_surface = carries(true) && !breast_surface;
    if let Some(shape) = glute_surface
        .then(|| profile.glute_size_shape(1.0))
        .flatten()
    {
        return JigglePromotion::plain(promote_glute_mesh(
            source_vertices,
            source_indices,
            profile,
            tuning,
            &shape,
            lod_index,
        ));
    }
    let first = subdivide_jiggle_region(source_vertices, source_indices, 0.025, profile, tuning);
    if lod_index == 1 {
        return JigglePromotion::plain(first);
    }
    if breast_surface {
        let weight = |vertex: &Ghoul2GpuVertex| jiggle_promotion_weight(vertex, profile, tuning);
        let reference = subdivide_weighted(
            &first.0,
            &first.1,
            BREAST_REFERENCE_PROMOTION_THRESHOLD,
            &weight,
            None,
        );
        let reduced = subdivide_weighted(
            &first.0,
            &first.1,
            BREAST_SECOND_PROMOTION_THRESHOLD,
            &weight,
            Some((&weight, BREAST_REFERENCE_PROMOTION_THRESHOLD)),
        );
        return JigglePromotion {
            geometry: reduced,
            breast_reference: Some(reference),
        };
    }
    JigglePromotion::plain(subdivide_jiggle_region(
        &first.0, &first.1, 0.18, profile, tuning,
    ))
}

fn promote_glute_mesh(
    source_vertices: &[Ghoul2GpuVertex],
    source_indices: &[u32],
    profile: &JiggleProfile,
    tuning: JiggleRemeshTuning,
    shape: &GluteShape,
    lod_index: usize,
) -> (Vec<Ghoul2GpuVertex>, Vec<u32>) {
    let weight = |vertex: &Ghoul2GpuVertex, round| {
        shape.remesh_weight(
            vertex.position,
            vertex.normal,
            jiggle_promotion_weight(vertex, profile, tuning),
            round,
            tuning.glute_bottom_z,
            tuning.glute_top_z,
        ) * profile.size_skin_gate(true, &vertex.bone_indices, &vertex.weights, vertex.weight_count)
    };
    let h_weight = |vertex: &Ghoul2GpuVertex| weight(vertex, false);
    let i_weight = |vertex: &Ghoul2GpuVertex| weight(vertex, true);
    // At the first threshold, I's extra patch is wholly inside H's selection.
    // Reusing this common first pass preserves H's existing bind geometry.
    let first = subdivide_weighted(source_vertices, source_indices, 0.025, &h_weight, None);
    let budget = source_indices.len().saturating_mul(3);
    if lod_index == 1 {
        return if first.1.len() <= budget {
            first
        } else {
            (source_vertices.to_vec(), source_indices.to_vec())
        };
    }
    for step in 0..=8 {
        let threshold = 0.80 + step as f32 * 0.025;
        let second = subdivide_weighted(
            &first.0,
            &first.1,
            threshold,
            &i_weight,
            Some((&h_weight, threshold)),
        );
        if second.1.len() <= budget {
            return second;
        }
    }
    // Other skins/overlays can have different density: enforce the same cap.
    if first.1.len() <= budget {
        first
    } else {
        (source_vertices.to_vec(), source_indices.to_vec())
    }
}

/// Light relaxation passes over the size displacement field (promoted core is
/// ~4x denser than stock, so this stays well inside one stock-triangle ring).
const JIGGLE_MASS_SMOOTH_ITERATIONS: usize = 3;

/// Bake the breast/glute size sliders into a promoted surface's bind pose.
/// Displacement is averaged per welded position so UV/hard-normal seams move
/// together. Positive glute size uses a smooth position field with waist/leg
/// fades; other size paths pin open boundaries to avoid cracks. Normals get the
/// delta between recomputed face normals after/before, preserving authored smoothing
/// at size 0 and tilts it with the new shape.
pub(in crate::cgame::player_presenter) fn inflate_jiggle_mass(
    vertices: &mut [Ghoul2GpuVertex],
    front_indices: &[u32],
    profile: &JiggleProfile,
    tuning: JiggleRemeshTuning,
) {
    if tuning.breast_size == 0.0 && tuning.glute_size == 0.0 {
        return;
    }
    let weld_key = |position: [f32; 3]| {
        (
            (position[0] * 256.0).round() as i32,
            (position[1] * 256.0).round() as i32,
            (position[2] * 256.0).round() as i32,
        )
    };
    let mut groups = HashMap::<(i32, i32, i32), u32>::new();
    let group_of = vertices
        .iter()
        .map(|vertex| {
            let next = groups.len() as u32;
            *groups.entry(weld_key(vertex.position)).or_insert(next)
        })
        .collect::<Vec<_>>();
    let group_count = groups.len();

    // Positive breast size shrink-wraps every vertex of a chest surface onto
    // its side's egg (undersides/sides included, not just the motion mask).
    let egg_wrap = tuning.breast_size > 0.0
        && vertices
            .iter()
            .any(|vertex| vertex.jiggle_weight > 0.0 && profile.is_breast_region(vertex.jiggle_region));
    // Candidate I is prepared once and evaluated for every vertex of a hips
    // surface, including its cloth/skin openings and original clothing pieces.
    let glute_shape = if (!egg_wrap || profile.uses_source_anatomy())
        && tuning.glute_size > 0.0
        && vertices.iter().any(|vertex| {
            vertex.jiggle_weight > 0.0 && profile.is_glute_region(vertex.jiggle_region)
        })
    {
        profile.glute_size_shape(tuning.glute_size)
    } else {
        None
    };

    let mut displacement = vec![Vec3::ZERO; group_count];
    let mut members = vec![0u32; group_count];
    // The nipple pass runs after smoothing: also track which welded vertices
    // are genuinely breast-skinned so adjacent hands cannot get a nipple bump.
    let mut breast_skin_gates = vec![0.0f32; group_count];
    for (vertex, &group) in vertices.iter().zip(&group_of) {
        let offset = if profile.uses_source_anatomy() && (egg_wrap || glute_shape.is_some()) {
            let breast_gate = profile.size_skin_gate(
                false, &vertex.bone_indices, &vertex.weights, vertex.weight_count,
            );
            let glute_gate = profile.size_skin_gate(
                true, &vertex.bone_indices, &vertex.weights, vertex.weight_count,
            );
            breast_skin_gates[group as usize] =
                breast_skin_gates[group as usize].max(breast_gate);
            // Source bodies often share one mesh for both regions; blend their
            // *independent* size fields, rather than letting a breast wrap
            // suppress the glute slider for the whole combined surface.
            let mut delta = Vec3::ZERO;
            if egg_wrap {
                delta += Vec3::from_array(profile.breast_egg_displacement(
                    vertex.position, vertex.normal, tuning.breast_size,
                )) * breast_gate;
            }
            if let Some(shape) = &glute_shape {
                delta += Vec3::from_array(shape.displacement(vertex.position)) * glute_gate;
            }
            delta.to_array()
        } else if egg_wrap {
            profile.breast_egg_displacement(vertex.position, vertex.normal, tuning.breast_size)
        } else if let Some(shape) = &glute_shape {
            shape.displacement(vertex.position)
        } else if vertex.jiggle_region != u32::MAX {
            profile.size_displacement(
                vertex.jiggle_region,
                vertex.jiggle_weight,
                vertex.position,
                vertex.normal,
                tuning,
            )
        } else {
            continue;
        };
        displacement[group as usize] += Vec3::from_array(offset);
        members[group as usize] += 1;
    }
    for (value, &count) in displacement.iter_mut().zip(&members) {
        if count > 1 {
            *value /= count as f32;
        }
    }

    let triangles = front_indices
        .chunks_exact(3)
        .filter(|triangle| triangle.iter().all(|&index| (index as usize) < vertices.len()))
        .map(|triangle| {
            [
                group_of[triangle[0] as usize],
                group_of[triangle[1] as usize],
                group_of[triangle[2] as usize],
            ]
        })
        .collect::<Vec<_>>();
    let mut edge_users = HashMap::<(u32, u32), u32>::new();
    for triangle in &triangles {
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            if a != b {
                *edge_users.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
    }
    let mut pinned = vec![false; group_count];
    let mut neighbours = vec![Vec::<u32>::new(); group_count];
    for (&(a, b), &users) in &edge_users {
        if users == 1 && glute_shape.is_none() {
            pinned[a as usize] = true;
            pinned[b as usize] = true;
        }
        neighbours[a as usize].push(b);
        neighbours[b as usize].push(a);
    }
    for (value, &pin) in displacement.iter_mut().zip(&pinned) {
        if pin {
            *value = Vec3::ZERO;
        }
    }
    let peak_before = displacement
        .iter()
        .map(|value| value.length())
        .fold(0.0f32, f32::max);
    if peak_before <= 0.0 {
        return;
    }

    // Relax the displacement field over the welded surface: rounds the dome,
    // removes facet steps inherited from the low-res base and the mask's
    // cutoffs, and lets the bulge ease a few rings into its surroundings.
    // Pinned seams stay at zero; the peak is restored afterwards so the slider
    // keeps meaning "units at the peak".
    // I is already shaped and faired. Relaxing or peak-rescaling its fitted field would
    // flatten the approved upper fullness and move the waist/leg fades.
    let smooth_iterations = if glute_shape.is_some() {
        0
    } else {
        JIGGLE_MASS_SMOOTH_ITERATIONS
    };
    for _ in 0..smooth_iterations {
        let previous = displacement.clone();
        for group in 0..group_count {
            if pinned[group] || neighbours[group].is_empty() {
                continue;
            }
            let average = neighbours[group]
                .iter()
                .map(|&other| previous[other as usize])
                .sum::<Vec3>()
                / neighbours[group].len() as f32;
            displacement[group] = previous[group].lerp(average, 0.5);
        }
    }
    let peak_after = displacement
        .iter()
        .map(|value| value.length())
        .fold(0.0f32, f32::max);
    if glute_shape.is_none() && peak_after > 0.0001 {
        let restore = peak_before / peak_after;
        for value in &mut displacement {
            *value *= restore;
        }
    }

    let mut bind = vec![Vec3::ZERO; group_count];
    let mut authored = vec![Vec3::ZERO; group_count];
    for (vertex, &group) in vertices.iter().zip(&group_of) {
        bind[group as usize] = Vec3::from_array(vertex.position);
        authored[group as usize] += Vec3::from_array(vertex.normal);
    }
    // Nipple bump goes on after smoothing (which would flatten it), at the
    // wrapped positions. Seams stay pinned.
    if egg_wrap {
        for group in 0..group_count {
            if pinned[group] {
                continue;
            }
            let wrapped = bind[group] + displacement[group];
            let breast_gate = if profile.uses_source_anatomy() {
                breast_skin_gates[group]
            } else {
                1.0
            };
            if breast_gate > 0.0 {
                displacement[group] += Vec3::from_array(
                    profile.breast_nipple_displacement(wrapped.to_array(), tuning.breast_size),
                ) * breast_gate;
            }
        }
    }
    let mut before = vec![Vec3::ZERO; group_count];
    let mut after = vec![Vec3::ZERO; group_count];
    for triangle in &triangles {
        let [a, b, c] = (*triangle).map(|group| group as usize);
        let face_before = (bind[b] - bind[a]).cross(bind[c] - bind[a]);
        let (pa, pb, pc) = (
            bind[a] + displacement[a],
            bind[b] + displacement[b],
            bind[c] + displacement[c],
        );
        let face_after = (pb - pa).cross(pc - pa);
        for group in [a, b, c] {
            before[group] += face_before;
            after[group] += face_after;
        }
    }

    for (vertex, &group) in vertices.iter_mut().zip(&group_of) {
        let group = group as usize;
        let offset = displacement[group];
        if offset == Vec3::ZERO {
            continue;
        }
        vertex.position = (Vec3::from_array(vertex.position) + offset).to_array();
        let mut normal_before = before[group].normalize_or_zero();
        let mut normal_after = after[group].normalize_or_zero();
        if normal_before == Vec3::ZERO || normal_after == Vec3::ZERO {
            continue;
        }
        // GLM winding is flipped for the renderer; orient the recomputed fan
        // with the authored normals before taking the delta.
        if normal_before.dot(authored[group]) < 0.0 {
            normal_before = -normal_before;
            normal_after = -normal_after;
        }
        let normal = (Vec3::from_array(vertex.normal) + (normal_after - normal_before))
            .normalize_or_zero();
        if normal != Vec3::ZERO {
            vertex.normal = normal.to_array();
        }
    }
}

fn breast_bind_key(vertex: &Ghoul2GpuVertex) -> ([u32; 3], [u32; 3]) {
    (
        vertex.position.map(f32::to_bits),
        vertex.normal.map(f32::to_bits),
    )
}

// A material split through the breast is an internal seam, not a fixed chest
// attachment. Detect matching cloth/skin pieces by their authored surface
// names and shared boundary edges; no model-specific shape or path checks.
const BREAST_SEAM_MIN_WEIGHT: f32 = 0.25;
type BreastSeamEdge = ([i32; 3], [i32; 3]);
type BreastBake = HashMap<([u32; 3], [u32; 3]), ([f32; 3], [f32; 3])>;

fn breast_boundary_edges(
    surface: &GlmSurface,
    lod_index: usize,
    profile: &JiggleProfile,
) -> HashMap<BreastSeamEdge, f32> {
    let points = surface
        .vertices
        .iter()
        .enumerate()
        .map(|(index, vertex)| {
            let (region, weight, _) =
                profile.gpu_vertex_binding(lod_index, surface.surface_index, index);
            let weight = if profile.is_breast_region(region) {
                weight
            } else {
                0.0
            };
            (
                vertex.position.map(|value| (value * 256.0).round() as i32),
                weight,
            )
        })
        .collect::<Vec<_>>();
    let mut edges = HashMap::<BreastSeamEdge, (u32, f32)>::new();
    for triangle in &surface.triangles {
        if triangle.iter().any(|&index| index as usize >= points.len()) {
            continue;
        }
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            let (pa, wa) = points[a as usize];
            let (pb, wb) = points[b as usize];
            if pa == pb {
                continue;
            }
            let key = if pa < pb { (pa, pb) } else { (pb, pa) };
            let entry = edges.entry(key).or_default();
            entry.0 += 1;
            entry.1 = entry.1.max(wa.min(wb));
        }
    }
    edges
        .into_iter()
        .filter_map(|(edge, (users, weight))| {
            (users == 1).then_some((edge, weight))
        })
        .collect()
}

fn breast_seam_partner<'a>(
    glm: &'a GlmModel,
    lod_index: usize,
    surface_index: usize,
    profile: &JiggleProfile,
    tuning: JiggleRemeshTuning,
) -> Option<&'a GlmSurface> {
    if tuning.breast_size <= 0.0 || !profile.gpu_supported() {
        return None;
    }
    let name = glm.hierarchy.get(surface_index)?.name.to_ascii_lowercase();
    let partner_name = name
        .strip_suffix("_skin")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{name}_skin"));
    let partner_index = glm
        .hierarchy
        .iter()
        .position(|entry| entry.name.eq_ignore_ascii_case(&partner_name))?;
    if !profile.affects_surface(lod_index, surface_index)
        || !profile.affects_surface(lod_index, partner_index)
    {
        return None;
    }
    // Classify the material split on LOD0. Coarser LODs can remove the
    // strongly weighted endpoints, but must keep the same connected bake.
    let base = &glm.lods.first()?.surfaces;
    let base_surface = base
        .iter()
        .find(|entry| entry.surface_index == surface_index)?;
    let base_partner = base
        .iter()
        .find(|entry| entry.surface_index == partner_index)?;
    let boundary = breast_boundary_edges(base_surface, 0, profile);
    let other = breast_boundary_edges(base_partner, 0, profile);
    if !boundary.iter().any(|(edge, &weight)| {
        weight > BREAST_SEAM_MIN_WEIGHT
            && other
                .get(edge)
                .is_some_and(|&weight| weight > BREAST_SEAM_MIN_WEIGHT)
    }) {
        return None;
    }
    let surfaces = &glm.lods.get(lod_index)?.surfaces;
    let surface = surfaces
        .iter()
        .find(|entry| entry.surface_index == surface_index)?;
    let partner = surfaces
        .iter()
        .find(|entry| entry.surface_index == partner_index)?;
    let boundary = breast_boundary_edges(surface, lod_index, profile);
    let other = breast_boundary_edges(partner, lod_index, profile);
    boundary
        .keys()
        .any(|edge| other.contains_key(edge))
        .then_some(partner)
}

fn bake_joined_breast_surfaces(
    surface: &GlmSurface,
    partner: &GlmSurface,
    lod_index: usize,
    profile: &JiggleProfile,
    tuning: JiggleRemeshTuning,
) -> Result<BreastBake, String> {
    let mut pair = [surface, partner];
    // Both requests for this pair must traverse the same welded graph/order.
    pair.sort_by_key(|surface| surface.surface_index);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for source in pair {
        if source.vertices.len() != source.texcoords.len() {
            return Err(format!(
                "GLM surface {} has mismatched vertices/texcoords",
                source.surface_index
            ));
        }
        let raw = (0..source.vertices.len())
            .map(|index| gpu_vertex_from_glm(source, index, Some(profile), lod_index))
            .collect::<Result<Vec<_>, _>>()?;
        let front = append_surface_indices(source, false)?;
        let JigglePromotion {
            geometry,
            breast_reference,
        } = promote_jiggle_mesh(&raw, &front, profile, tuning, lod_index);
        // LOD0 uses the same temporary dense bake as the reduced breast mesh.
        // LOD1/2 use only their own one/no-pass topology.
        let (bake_vertices, bake_indices) = breast_reference.unwrap_or(geometry);
        let offset = u32::try_from(vertices.len())
            .map_err(|_| "joined breast mesh exceeds GPU u32".to_owned())?;
        indices.extend(bake_indices.into_iter().map(|index| index + offset));
        vertices.extend(bake_vertices);
    }
    let keys = vertices.iter().map(breast_bind_key).collect::<Vec<_>>();
    // Welding closes the common edge, so the existing bake pins only exterior
    // attachments and relaxes a single displacement field across the material.
    inflate_jiggle_mass(&mut vertices, &indices, profile, tuning);
    Ok(keys
        .into_iter()
        .zip(vertices)
        .map(|(key, vertex)| (key, (vertex.position, vertex.normal)))
        .collect())
}

type JoinedBreastBakes = HashMap<(usize, usize), Arc<BreastBake>>;

/// Registration-only reference fields. Each pair is baked once and both
/// material pieces sample the same positions/normals; none of this extra
/// geometry is kept in the model asset or uploaded to the GPU.
pub(in crate::cgame::player_presenter) fn build_joined_breast_bakes(
    glm: &GlmModel,
    profile: Option<&JiggleProfile>,
    tuning: JiggleRemeshTuning,
) -> Result<JoinedBreastBakes, String> {
    let mut bakes = JoinedBreastBakes::new();
    let Some(profile) =
        profile.filter(|profile| tuning.breast_size > 0.0 && profile.gpu_supported())
    else {
        return Ok(bakes);
    };
    for (lod_index, lod) in glm.lods.iter().enumerate() {
        for surface in &lod.surfaces {
            let key = (lod_index, surface.surface_index);
            if bakes.contains_key(&key) {
                continue;
            }
            let Some(partner) =
                breast_seam_partner(glm, lod_index, surface.surface_index, profile, tuning)
            else {
                continue;
            };
            let bake = Arc::new(bake_joined_breast_surfaces(
                surface, partner, lod_index, profile, tuning,
            )?);
            bakes.insert(key, Arc::clone(&bake));
            bakes.insert((lod_index, partner.surface_index), bake);
        }
    }
    Ok(bakes)
}

pub(in crate::cgame::player_presenter) fn build_gpu_mesh_source(
    model_qpath: &str,
    lod_index: usize,
    surface: &GlmSurface,
    two_sided: bool,
    jiggle_profile: Option<&JiggleProfile>,
    jiggle_remesh: JiggleRemeshTuning,
    joined_breast_bake: Option<&BreastBake>,
) -> Result<Arc<Ghoul2GpuMeshSource>, String> {
    if surface.vertices.len() != surface.texcoords.len() {
        return Err(format!(
            "GLM surface {} has {} vertices but {} texcoords",
            surface.surface_index,
            surface.vertices.len(),
            surface.texcoords.len()
        ));
    }
    let mut vertices = Vec::with_capacity(surface.vertices.len());
    for vertex_index in 0..surface.vertices.len() {
        vertices.push(gpu_vertex_from_glm(
            surface,
            vertex_index,
            jiggle_profile,
            lod_index,
        )?);
    }

    let cpu_indices = append_surface_indices(surface, two_sided)?;
    let base_vertices = Arc::new(vertices.clone());
    let promote = jiggle_profile.is_some_and(|profile| {
        profile.gpu_supported() && profile.affects_surface(lod_index, surface.surface_index)
    });
    let (vertices, mut indices) = if promote {
        // Promote only the front winding; append mirrored triangles afterwards
        // for the rare two-sided material so midpoint topology is shared.
        let front_count = surface.triangles.len() * 3;
        let profile = jiggle_profile.expect("promoted jiggle mesh requires profile");
        let promotion = promote_jiggle_mesh(
            &vertices,
            &cpu_indices[..front_count],
            profile,
            jiggle_remesh,
            lod_index,
        );
        let (mut vertices, mut promoted) = promotion.geometry;
        if let Some(baked) = joined_breast_bake.filter(|baked| {
            vertices
                .iter()
                .all(|vertex| baked.contains_key(&breast_bind_key(vertex)))
        }) {
            // Only the baked geometry changes. UVs, skin and motion bindings
            // stay on the existing reduced draw vertices.
            for vertex in &mut vertices {
                let &(position, normal) = &baked[&breast_bind_key(vertex)];
                vertex.position = position;
                vertex.normal = normal;
            }
        } else if let Some((mut reference_vertices, reference_indices)) = promotion.breast_reference
        {
            let keys = reference_vertices
                .iter()
                .map(breast_bind_key)
                .collect::<Vec<_>>();
            inflate_jiggle_mass(
                &mut reference_vertices,
                &reference_indices,
                profile,
                jiggle_remesh,
            );
            let baked = keys
                .into_iter()
                .zip(reference_vertices.iter())
                .map(|(key, vertex)| (key, (vertex.position, vertex.normal)))
                .collect::<HashMap<_, _>>();
            // Retained midpoint geometry makes every key present. Keep a
            // regular bake fallback for unusual authored seam/normal variants.
            if !vertices
                .iter()
                .all(|vertex| baked.contains_key(&breast_bind_key(vertex)))
            {
                inflate_jiggle_mass(&mut vertices, &promoted, profile, jiggle_remesh);
            } else {
                for vertex in &mut vertices {
                    let &(position, normal) = &baked[&breast_bind_key(vertex)];
                    vertex.position = position;
                    vertex.normal = normal;
                }
            }
        } else {
            inflate_jiggle_mass(&mut vertices, &promoted, profile, jiggle_remesh);
        }
        if two_sided {
            let promoted_front = promoted.len();
            for triangle in 0..promoted_front / 3 {
                let base = triangle * 3;
                promoted.extend_from_slice(&[
                    promoted[base],
                    promoted[base + 2],
                    promoted[base + 1],
                ]);
            }
        }
        (vertices, promoted)
    } else {
        (vertices, cpu_indices.clone())
    };

    // Keep capacity tight after midpoint generation; these Arcs live with the
    // model asset for its whole residency.
    indices.shrink_to_fit();
    let base_key = Arc::<str>::from(format!(
        "{}#lod{}#surface{}{}",
        model_qpath.replace('\\', "/").to_ascii_lowercase(),
        lod_index,
        surface.surface_index,
        if two_sided { "#twosided" } else { "" },
    ));
    let base_key = if jiggle_remesh.body_shape.is_neutral() && jiggle_remesh.body_details.is_neutral() {
        base_key
    } else {
        Arc::<str>::from(format!("{base_key}#profile_universal_v1_{:016x}",
            jka_assets::body_shape::anny::fingerprint(jiggle_remesh.body_shape, jiggle_remesh.body_details)))
    };
    let key = if promote {
        Arc::<str>::from(format!(
            "{base_key}#jiggle_source_skin_guard_v1{}#rb{:08x}#rt{:08x}#gb{:08x}#gt{:08x}#bs{:08x}#gs{:08x}",
            if joined_breast_bake.is_some() { "#breast_joined_v1" } else { "" },
            jiggle_remesh.breast_bottom_z.to_bits(),
            jiggle_remesh.breast_top_z.to_bits(),
            jiggle_remesh.glute_bottom_z.to_bits(),
            jiggle_remesh.glute_top_z.to_bits(),
            jiggle_remesh.breast_size.to_bits(),
            jiggle_remesh.glute_size.to_bits(),
        ))
    } else {
        Arc::clone(&base_key)
    };
    let vertices = if promote {
        Arc::new(vertices)
    } else {
        Arc::clone(&base_vertices)
    };
    let cpu_indices = Arc::new(cpu_indices);
    let indices = if promote {
        Arc::new(indices)
    } else {
        Arc::clone(&cpu_indices)
    };
    Ok(Arc::new(Ghoul2GpuMeshSource {
        base_key,
        base_vertices,
        cpu_indices,
        key,
        vertices,
        indices,
    }))
}

pub(in crate::cgame::player_presenter) fn build_lod_gpu_meshes(
    model_qpath: &str,
    glm: &GlmModel,
    surface_index: usize,
    two_sided: bool,
    jiggle_profile: Option<&JiggleProfile>,
    jiggle_remesh: JiggleRemeshTuning,
    joined_breast_bakes: &JoinedBreastBakes,
) -> Result<Vec<Option<Arc<Ghoul2GpuMeshSource>>>, String> {
    glm.lods
        .iter()
        .enumerate()
        .map(|(lod_index, lod)| {
            lod.surfaces
                .iter()
                .find(|surface| surface.surface_index == surface_index)
                .map(|surface| {
                    build_gpu_mesh_source(
                        model_qpath,
                        lod_index,
                        surface,
                        two_sided,
                        jiggle_profile,
                        jiggle_remesh,
                        joined_breast_bakes
                            .get(&(lod_index, surface_index))
                            .map(Arc::as_ref),
                    )
                })
                .transpose()
        })
        .collect()
}

pub(in crate::cgame::player_presenter) fn gpu_bones_from_pose(
    pose: &[Matrix3x4],
) -> Arc<Vec<Ghoul2GpuBone>> {
    Arc::new(
        pose.iter()
            .map(|matrix| Ghoul2GpuBone {
                row0: matrix[0],
                row1: matrix[1],
                row2: matrix[2],
            })
            .collect(),
    )
}

/// Marker carried by errors that only mean "still loading". Presentation code
/// treats it as "skip this frame", never as a failure to report.
pub(in crate::cgame::player_presenter) const ASSET_PENDING: &str = "asset loading";

pub(in crate::cgame::player_presenter) fn is_asset_pending(error: &str) -> bool {
    error.contains(ASSET_PENDING)
}

/// OpenJK cache key for a player's model + skin registration.
pub(in crate::cgame::player_presenter) fn player_model_key(
    info: &crate::cgame::ClientInfo,
) -> String {
    format!("{}|{}", info.model_qpath(), info.skin_qpath()).to_ascii_lowercase()
}

pub(in crate::cgame::player_presenter) fn sibling_default_skin_qpath(
    model_qpath: &str,
) -> Option<String> {
    let normalized = model_qpath.replace('\\', "/");
    let slash = normalized.rfind('/')?;
    Some(format!("{}model_default.skin", &normalized[..=slash]))
}

pub(in crate::cgame::player_presenter) fn static_model_key(
    model_qpath: &str,
    custom_skin: Option<&str>,
    preview_fallback: bool,
) -> String {
    format!(
        "{}|{}|preview_fallback={}",
        model_qpath,
        custom_skin.unwrap_or(""),
        u8::from(preview_fallback),
    )
    .to_ascii_lowercase()
}

#[cfg(test)]
mod source_morph_tests {
    use super::*;

    fn point(position: [f32; 3], bone: u32, glute: bool) -> Ghoul2GpuVertex {
        Ghoul2GpuVertex {
            position,
            normal: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            bone_indices: [bone, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
            weight_count: 1,
            jiggle_region: if glute { 0 } else { u32::MAX },
            jiggle_weight: if glute { 1.0 } else { 0.0 },
            jiggle_coord: 0.0,
        }
    }

    #[test]
    fn positive_glute_size_does_not_move_nearby_hand_on_a_shared_source_surface() {
        let profile = JiggleProfile::source_size_test_profile();
        // Different triangles on one mesh. The hand occupies the same volume
        // as the glute at bind pose, but has unrelated GLA skin weights.
        let mut vertices = vec![
            point([2.98403537, 3.70633138, 40.7899405], 0, true),
            point([3.4, 3.6, 40.5], 0, true),
            point([2.7, 3.9, 40.0], 0, true),
            point([3.02, 3.76, 40.73], 1, false),
            point([3.45, 3.65, 40.45], 1, false),
            point([2.75, 3.95, 39.95], 1, false),
        ];
        let original = vertices.iter().map(|v| v.position).collect::<Vec<_>>();
        let indices = [0, 1, 2, 3, 4, 5];
        let tuning = JiggleRemeshTuning { glute_size: 4.0, ..Default::default() };
        inflate_jiggle_mass(&mut vertices, &indices, &profile, tuning);
        assert!(vertices[..3].iter().zip(&original[..3]).any(|(v, p)| v.position != *p),
            "hip-skinned points should still respond to the size slider");
        for i in 3..6 {
            assert_eq!(vertices[i].position, original[i], "hand vertex {i} must remain fixed");
        }
    }
}
