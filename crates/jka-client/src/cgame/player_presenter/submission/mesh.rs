//! Submission mesh.
use crate::cgame::player_presenter::{
    Arc, Ghoul2GpuBone, Ghoul2GpuMeshSource, Ghoul2GpuVertex, GlmModel, GlmSurface, HashMap,
    JiggleProfile, Matrix3x4, Vec3,
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

pub(in crate::cgame::player_presenter) fn jiggle_promotion_weight(vertex: &Ghoul2GpuVertex) -> f32 {
    if vertex.jiggle_coord.abs() < 2.0 {
        // Match the default live lower-edge trim while deciding where extra
        // topology is worth caching. The automatic glute footprint now keeps a
        // little upper-thigh participation instead of cutting off at the crease.
        let edge0 = -0.70;
        let edge1 = -0.15;
        let t = ((vertex.jiggle_coord - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        return vertex.jiggle_weight * (t * t * (3.0 - 2.0 * t));
    }
    vertex.jiggle_weight
}

pub(in crate::cgame::player_presenter) fn subdivide_jiggle_region(
    source_vertices: &[Ghoul2GpuVertex],
    source_indices: &[u32],
    threshold: f32,
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
                    .is_some_and(|vertex| jiggle_promotion_weight(vertex) > threshold)
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
    let core = triangles
        .iter()
        .enumerate()
        .map(|(triangle_index, triangle)| {
            if !selected[triangle_index] {
                return false;
            }
            [
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
        .collect::<Vec<_>>();

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
        let curve_position = users.len() == 2 && users.iter().all(|&index| core[index]);
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

/// Two cached, local Phong-style subdivision passes. The first pass promotes
/// only the interior of the soft mask while preserving a one-triangle linear
/// guard band against stock topology. A much smaller strong-weight core gets a
/// second pass, so average density lands between the old 4x and broad 16x versions.
pub(in crate::cgame::player_presenter) fn promote_jiggle_mesh(
    source_vertices: &[Ghoul2GpuVertex],
    source_indices: &[u32],
) -> (Vec<Ghoul2GpuVertex>, Vec<u32>) {
    let (vertices, indices) = subdivide_jiggle_region(source_vertices, source_indices, 0.025);
    subdivide_jiggle_region(&vertices, &indices, 0.18)
}

pub(in crate::cgame::player_presenter) fn build_gpu_mesh_source(
    model_qpath: &str,
    lod_index: usize,
    surface: &GlmSurface,
    two_sided: bool,
    jiggle_profile: Option<&JiggleProfile>,
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
        let (vertices, mut promoted) = promote_jiggle_mesh(&vertices, &cpu_indices[..front_count]);
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
    let key = if promote {
        Arc::<str>::from(format!("{base_key}#jiggle_local_guard_v2"))
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
