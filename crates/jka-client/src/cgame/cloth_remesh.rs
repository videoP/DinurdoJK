//! Presentation-only densification of auto-promoted Ghoul2 cloth surfaces.
//!
//! Stock capes/robes are authored for rigid skinning and are far too coarse for
//! a particle cloth: folds cannot form between the original vertices, and a
//! limb can pass between particles because contacts are per particle. Split
//! long edges of the bind-pose surface before skinning so the solver, contacts
//! and the rendered garment share one finer topology. Skin weights and UVs are
//! interpolated; nothing is reweighted or expanded.
//!
//! The result is deterministic for a given surface. Welded position copies
//! (UV/normal seams) receive bit-identical midpoints, and open boundaries use
//! linear midpoints, so cross-surface garment stitching and attachments to
//! non-cloth body surfaces keep their authored edges.

use glam::Vec3;
use jka_assets::ghoul2::{GlmSurface, GlmVertex, GlmWeight};
use std::collections::HashMap;

/// Edges at or below this bind-pose length (JKA units) are left alone, so an
/// already-dense region is not refined further.
const TARGET_EDGE_JKA: f32 = 2.5;
const MAX_LEVELS: usize = 2;
/// Per-surface budget. A refinement level that would exceed it is skipped.
const MAX_TRIANGLES: usize = 3072;
/// Ghoul2 skinning supports four weights per vertex.
const MAX_WEIGHTS: usize = 4;
/// Matches the cloth solver's intra-surface weld quantization.
const WELD_SCALE: f32 = 1024.0;
/// Cubic (PN-style) edge bulge is applied only across smooth welded edges.
const SMOOTH_NORMAL_DOT: f32 = 0.5;
const MAX_BULGE_FRACTION: f32 = 0.15;

/// Returns `None` when the surface is invalid or already fine enough.
pub(crate) fn remesh_cloth_surface(surface: &GlmSurface) -> Option<GlmSurface> {
    if surface.vertices.len() != surface.texcoords.len()
        || surface.triangles.is_empty()
        || surface
            .triangles
            .iter()
            .flatten()
            .any(|&index| index as usize >= surface.vertices.len())
    {
        return None;
    }
    let mut current: Option<GlmSurface> = None;
    for _ in 0..MAX_LEVELS {
        let source = current.as_ref().unwrap_or(surface);
        match split_long_edges(source) {
            Some(next) if next.triangles.len() <= MAX_TRIANGLES => current = Some(next),
            _ => break,
        }
    }
    current
}

fn weld_key(p: [f32; 3]) -> (i32, i32, i32) {
    (
        (p[0] * WELD_SCALE).round() as i32,
        (p[1] * WELD_SCALE).round() as i32,
        (p[2] * WELD_SCALE).round() as i32,
    )
}

fn split_long_edges(surface: &GlmSurface) -> Option<GlmSurface> {
    // Welded position groups: canonical position and averaged normal.
    let mut group_ids = HashMap::<(i32, i32, i32), u32>::new();
    let mut group_position = Vec::<Vec3>::new();
    let mut group_normal = Vec::<Vec3>::new();
    let group_of = surface
        .vertices
        .iter()
        .map(|vertex| {
            let next = group_position.len() as u32;
            let group = *group_ids.entry(weld_key(vertex.position)).or_insert(next);
            if group == next {
                group_position.push(Vec3::from_array(vertex.position));
                group_normal.push(Vec3::ZERO);
            }
            group_normal[group as usize] += Vec3::from_array(vertex.normal).normalize_or_zero();
            group
        })
        .collect::<Vec<_>>();
    for normal in &mut group_normal {
        *normal = normal.normalize_or_zero();
    }

    let ordered = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
    let mut welded_use = HashMap::<(u32, u32), u32>::new();
    for triangle in &surface.triangles {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (ga, gb) = (group_of[triangle[a] as usize], group_of[triangle[b] as usize]);
            if ga != gb {
                *welded_use.entry(ordered(ga, gb)).or_default() += 1;
            }
        }
    }

    let mut vertices = surface.vertices.clone();
    let mut texcoords = surface.texcoords.clone();
    let mut midpoints = HashMap::<(u32, u32), u32>::new();
    for triangle in &surface.triangles {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let key = ordered(triangle[a], triangle[b]);
            if midpoints.contains_key(&key) {
                continue;
            }
            let (ga, gb) = (group_of[key.0 as usize], group_of[key.1 as usize]);
            if ga == gb {
                continue;
            }
            // Order by group so welded copies compute bit-identical positions.
            let (ga, gb) = ordered(ga, gb);
            let (pa, pb) = (group_position[ga as usize], group_position[gb as usize]);
            let length = pa.distance(pb);
            if !length.is_finite() || length <= TARGET_EDGE_JKA {
                continue;
            }
            let (na, nb) = (group_normal[ga as usize], group_normal[gb as usize]);
            let interior = welded_use.get(&(ga, gb)).copied().unwrap_or(0) >= 2;
            let mut position = (pa + pb) * 0.5;
            if interior && na.dot(nb) >= SMOOTH_NORMAL_DOT {
                // Cubic Bezier midpoint of the PN edge curve.
                let bulge = -((pb - pa).dot(na) * na + (pa - pb).dot(nb) * nb) / 8.0;
                position += bulge.clamp_length_max(length * MAX_BULGE_FRACTION);
            }
            let (va, vb) = (&surface.vertices[key.0 as usize], &surface.vertices[key.1 as usize]);
            let normal = (Vec3::from_array(va.normal).normalize_or_zero()
                + Vec3::from_array(vb.normal).normalize_or_zero())
            .try_normalize()
            .unwrap_or(Vec3::from_array(va.normal));
            let (ta, tb) = (surface.texcoords[key.0 as usize], surface.texcoords[key.1 as usize]);
            let index = u32::try_from(vertices.len()).ok()?;
            vertices.push(GlmVertex {
                normal: normal.to_array(),
                position: position.to_array(),
                weights: blend_weights(&va.weights, &vb.weights),
            });
            texcoords.push([(ta[0] + tb[0]) * 0.5, (ta[1] + tb[1]) * 0.5]);
            midpoints.insert(key, index);
        }
    }
    if midpoints.is_empty() {
        return None;
    }

    let mid = |a: u32, b: u32| midpoints.get(&ordered(a, b)).copied();
    let mut triangles = Vec::with_capacity(surface.triangles.len() * 4);
    for &[a, b, c] in &surface.triangles {
        // Same split table as the jiggle promotion: winding is preserved and a
        // partially split triangle shares its midpoints with its neighbours.
        match (mid(a, b), mid(b, c), mid(c, a)) {
            (None, None, None) => triangles.push([a, b, c]),
            (Some(ab), None, None) => triangles.extend([[a, ab, c], [ab, b, c]]),
            (None, Some(bc), None) => triangles.extend([[a, b, bc], [a, bc, c]]),
            (None, None, Some(ca)) => triangles.extend([[a, b, ca], [b, c, ca]]),
            (Some(ab), Some(bc), None) => triangles.extend([[ab, b, bc], [a, ab, bc], [a, bc, c]]),
            (None, Some(bc), Some(ca)) => triangles.extend([[bc, c, ca], [b, bc, ca], [b, ca, a]]),
            (Some(ab), None, Some(ca)) => triangles.extend([[ca, a, ab], [c, ca, ab], [c, ab, b]]),
            (Some(ab), Some(bc), Some(ca)) => {
                triangles.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]])
            }
        }
    }
    Some(GlmSurface {
        surface_index: surface.surface_index,
        vertices,
        texcoords,
        triangles,
        bone_references: surface.bone_references.clone(),
    })
}

/// Average two skin bindings, keep the strongest four and renormalize. The
/// strongest bone stays first, matching Ghoul2's first-weight normal path.
fn blend_weights(a: &[GlmWeight], b: &[GlmWeight]) -> Vec<GlmWeight> {
    let mut merged = Vec::<GlmWeight>::with_capacity(a.len() + b.len());
    for weight in a.iter().chain(b) {
        let half = weight.weight * 0.5;
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| existing.local_bone_index == weight.local_bone_index)
        {
            existing.weight += half;
        } else {
            merged.push(GlmWeight {
                local_bone_index: weight.local_bone_index,
                weight: half,
            });
        }
    }
    merged.sort_by(|x, y| {
        y.weight
            .total_cmp(&x.weight)
            .then(x.local_bone_index.cmp(&y.local_bone_index))
    });
    merged.truncate(MAX_WEIGHTS);
    let total: f32 = merged.iter().map(|weight| weight.weight).sum();
    if total > 1.0e-6 {
        for weight in &mut merged {
            weight.weight /= total;
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(cols: u32, rows: u32, spacing: f32) -> GlmSurface {
        let mut vertices = Vec::new();
        let mut texcoords = Vec::new();
        for row in 0..=rows {
            for col in 0..=cols {
                vertices.push(GlmVertex {
                    normal: [0.0, -1.0, 0.0],
                    position: [col as f32 * spacing, 0.0, row as f32 * spacing],
                    weights: vec![GlmWeight {
                        local_bone_index: (col % 2) as usize,
                        weight: 1.0,
                    }],
                });
                texcoords.push([col as f32 / cols as f32, row as f32 / rows as f32]);
            }
        }
        let mut triangles = Vec::new();
        for row in 0..rows {
            for col in 0..cols {
                let a = row * (cols + 1) + col;
                triangles.push([a, a + 1, a + cols + 1]);
                triangles.push([a + 1, a + cols + 2, a + cols + 1]);
            }
        }
        GlmSurface {
            surface_index: 3,
            vertices,
            texcoords,
            triangles,
            bone_references: vec![0, 1],
        }
    }

    #[test]
    fn coarse_cloth_is_refined_with_normalized_weights() {
        let source = grid(3, 3, 8.0);
        let refined = remesh_cloth_surface(&source).expect("coarse grid is refined");
        assert_eq!(refined.triangles.len(), source.triangles.len() * 16);
        assert_eq!(refined.vertices.len(), refined.texcoords.len());
        for vertex in &refined.vertices {
            let sum: f32 = vertex.weights.iter().map(|weight| weight.weight).sum();
            assert!((sum - 1.0).abs() < 1.0e-5);
            assert!(vertex.weights.len() <= MAX_WEIGHTS);
        }
    }

    #[test]
    fn dense_cloth_is_left_alone() {
        assert!(remesh_cloth_surface(&grid(3, 3, 2.0)).is_none());
    }
}
