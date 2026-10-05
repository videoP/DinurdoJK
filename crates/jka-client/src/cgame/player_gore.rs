//! Ghoul2 "gore": burn marks on player models (`EV_GHOUL2_MARK`, `CG_AddGhoul2Mark`).
//!
//! OpenJK projects a decal shader onto the skinned mesh at a hit point and keeps
//! it attached through the animation. Here the GPU-skinned surfaces of the
//! target's last presented frame are skinned once on the CPU to find the
//! triangles near the hit. Those triangles keep the *bind-pose* vertices with
//! UVs replaced by a planar projection, so each later frame draws them through
//! the same GPU skinning as the body and the mark follows the animation.

use std::sync::Arc;

use crate::renderer::{Ghoul2GpuBone, Ghoul2GpuVertex};

/// One GPU-skinned body surface of a presented frame.
#[derive(Clone)]
pub(crate) struct GpuSnap {
    pub mesh_key: Arc<str>,
    pub vertices: Arc<Vec<Ghoul2GpuVertex>>,
    pub indices: Arc<Vec<u32>>,
    pub bones: Arc<Vec<Ghoul2GpuBone>>,
    pub jiggle_offsets: [[f32; 4]; 4],
    pub axis: [[f32; 3]; 3],
    pub origin: [f32; 3],
}

/// The decal geometry for one mesh: shared vertices with projected UVs plus the
/// subset of triangles that carry the mark.
#[derive(Clone)]
pub(crate) struct GoreSurface {
    pub base_key: Arc<str>,
    pub key: Arc<str>,
    pub vertices: Arc<Vec<Ghoul2GpuVertex>>,
    pub indices: Arc<Vec<u32>>,
}

pub(crate) struct Gore {
    pub entity: u16,
    pub start_time: i32,
    pub life_ms: i32,
    pub shader: &'static str,
    pub surfaces: Vec<GoreSurface>,
}

fn transform_point(bone: &Ghoul2GpuBone, p: [f32; 3]) -> [f32; 3] {
    let dot = |row: [f32; 4]| row[0] * p[0] + row[1] * p[1] + row[2] * p[2] + row[3];
    [dot(bone.row0), dot(bone.row1), dot(bone.row2)]
}

fn transform_vector(bone: &Ghoul2GpuBone, v: [f32; 3]) -> [f32; 3] {
    let dot = |row: [f32; 4]| row[0] * v[0] + row[1] * v[1] + row[2] * v[2];
    [dot(bone.row0), dot(bone.row1), dot(bone.row2)]
}

/// `ghoul2_skin.wgsl` `skin_position` + `model_to_world`, and the weight-0 normal.
pub(crate) fn skin_vertex(vertex: &Ghoul2GpuVertex, snap: &GpuSnap) -> ([f32; 3], [f32; 3]) {
    let bone = |slot: usize| snap.bones.get(vertex.bone_indices[slot] as usize);
    let point = |slot: usize| bone(slot).map_or(vertex.position, |bone| transform_point(bone, vertex.position));
    let w = vertex.weights;
    let lerp3 = |terms: &[([f32; 3], f32)]| {
        let mut out = [0.0; 3];
        for (p, weight) in terms {
            for i in 0..3 {
                out[i] += p[i] * weight;
            }
        }
        out
    };
    let mut model = match vertex.weight_count {
        0 | 1 => point(0),
        2 => {
            let (p0, p1) = (point(0), point(1));
            lerp3(&[(p0, w[0]), (p1, 1.0 - w[0])])
        }
        3 => lerp3(&[(point(0), w[0]), (point(1), w[1]), (point(2), 1.0 - w[0] - w[1])]),
        _ => lerp3(&[(point(0), w[0]), (point(1), w[1]), (point(2), w[2]), (point(3), 1.0 - w[0] - w[1] - w[2])]),
    };
    if vertex.jiggle_weight > 0.0 && vertex.jiggle_region < 4 {
        let offset = snap.jiggle_offsets[vertex.jiggle_region as usize];
        let overall = snap.jiggle_offsets[0][3];
        let effective_weight = if vertex.jiggle_coord.abs() < 2.0 {
            let lift = snap.jiggle_offsets[3][3];
            let t = ((vertex.jiggle_coord - (-0.70 + lift))
                / ((-0.15 + lift) - (-0.70 + lift)))
                .clamp(0.0, 1.0);
            let vertical = t * t * (3.0 - 2.0 * t);
            vertex.jiggle_weight * overall * snap.jiggle_offsets[2][3] * vertical
        } else {
            vertex.jiggle_weight * overall * snap.jiggle_offsets[1][3]
        };
        for axis in 0..3 {
            model[axis] += offset[axis] * effective_weight;
        }
    }
    let model_normal = bone(0).map_or(vertex.normal, |bone| transform_vector(bone, vertex.normal));
    let to_world = |m: [f32; 3], with_origin: bool| -> [f32; 3] {
        std::array::from_fn(|i| {
            (if with_origin { snap.origin[i] } else { 0.0 })
                + snap.axis[0][i] * m[0]
                + snap.axis[1][i] * m[1]
                + snap.axis[2][i] * m[2]
        })
    };
    (to_world(model, true), to_world(model_normal, false))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length < 1.0e-6 { [0.0, 0.0, -1.0] } else { v.map(|c| c / length) }
}

/// Möller-Trumbore against the segment `start -> end`; returns the fraction.
fn segment_triangle(start: [f32; 3], end: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f32> {
    let dir = sub(end, start);
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1.0e-8 {
        return None;
    }
    let inv = 1.0 / det;
    let t_vec = sub(start, a);
    let u = dot(t_vec, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(t_vec, e1);
    let v = dot(dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (0.0..=1.0).contains(&t).then_some(t)
}

/// Build the decal for a hit at `hit` travelling along `dir`, half-extent `size`.
///
/// With `trace`, `hit` is only the start of a segment towards `hit + dir` (the
/// "special trace" of `EV_GHOUL2_MARK`'s eventParm) and the nearest surface hit
/// becomes the mark centre; no hit means no mark.
pub(crate) fn build_gore(
    snaps: &[GpuSnap],
    id: u32,
    start: [f32; 3],
    end: [f32; 3],
    trace: bool,
    size: f32,
) -> Vec<GoreSurface> {
    let skinned: Vec<Vec<([f32; 3], [f32; 3])>> = snaps
        .iter()
        .map(|snap| snap.vertices.iter().map(|vertex| skin_vertex(vertex, snap)).collect())
        .collect();
    let dir = normalize(sub(end, start));
    let hit = if trace {
        let mut best: Option<f32> = None;
        for (snap, world) in snaps.iter().zip(&skinned) {
            for tri in snap.indices.chunks_exact(3) {
                let at = |i: u32| world.get(i as usize).map(|(p, _)| *p);
                if let (Some(a), Some(b), Some(c)) = (at(tri[0]), at(tri[1]), at(tri[2])) {
                    if let Some(t) = segment_triangle(start, end, a, b, c) {
                        best = Some(best.map_or(t, |current| current.min(t)));
                    }
                }
            }
        }
        match best {
            Some(t) => std::array::from_fn(|i| start[i] + (end[i] - start[i]) * t),
            None => return Vec::new(),
        }
    } else {
        start
    };
    let up_hint = if dir[2].abs() > 0.95 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] };
    let right = normalize(cross(dir, up_hint));
    let up = cross(right, dir);
    let inv = 1.0 / (2.0 * size.max(0.1));

    let mut out = Vec::new();
    for (snap, world) in snaps.iter().zip(&skinned) {
        let mut vertices: Vec<Ghoul2GpuVertex> = snap.vertices.as_ref().clone();
        let mut uvs = vec![None::<[f32; 2]>; vertices.len()];
        let mut indices = Vec::new();
        for tri in snap.indices.chunks_exact(3) {
            let mut inside = false;
            let mut facing = 0.0;
            let mut ok = true;
            let mut tri_uv = [[0.0f32; 2]; 3];
            for (slot, &index) in tri.iter().enumerate() {
                let Some(&(position, normal)) = world.get(index as usize) else {
                    ok = false;
                    break;
                };
                let rel = sub(position, hit);
                let uv = [dot(rel, right) * inv + 0.5, dot(rel, up) * inv + 0.5];
                if dot(rel, dir).abs() > size * 1.5 {
                    ok = false;
                    break;
                }
                facing += dot(normal, dir);
                tri_uv[slot] = uv;
            }
            // The triangle's UV bounds overlap the decal square (a large triangle
            // can cover it without any corner being inside).
            if ok {
                let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
                for uv in &tri_uv {
                    for axis in 0..2 {
                        lo[axis] = lo[axis].min(uv[axis]);
                        hi[axis] = hi[axis].max(uv[axis]);
                    }
                }
                inside = hi[0] >= 0.0 && lo[0] <= 1.0 && hi[1] >= 0.0 && lo[1] <= 1.0;
            }
            // Only the side of the body the projectile came from.
            if ok && inside && facing / 3.0 < 0.2 {
                indices.extend_from_slice(tri);
                for (slot, &index) in tri.iter().enumerate() {
                    uvs[index as usize] = Some(tri_uv[slot]);
                }
            }
        }
        if indices.is_empty() {
            continue;
        }
        for (vertex, uv) in vertices.iter_mut().zip(&uvs) {
            if let Some(uv) = uv {
                vertex.uv = *uv;
            }
        }
        out.push(GoreSurface {
            base_key: Arc::clone(&snap.mesh_key),
            key: Arc::from(format!("{}#gore{id}", snap.mesh_key)),
            vertices: Arc::new(vertices),
            indices: Arc::new(indices),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex(position: [f32; 3]) -> Ghoul2GpuVertex {
        Ghoul2GpuVertex {
            position,
            normal: [0.0, -1.0, 0.0],
            uv: [0.0; 2],
            bone_indices: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
            weight_count: 1,
            jiggle_region: u32::MAX,
            jiggle_weight: 0.0,
            jiggle_coord: 4.0,
        }
    }

    /// A 40x40 quad in the XZ plane at y=0 facing -Y, on an identity bone.
    fn quad() -> GpuSnap {
        let bone = Ghoul2GpuBone { row0: [1.0, 0.0, 0.0, 0.0], row1: [0.0, 1.0, 0.0, 0.0], row2: [0.0, 0.0, 1.0, 0.0] };
        GpuSnap {
            mesh_key: Arc::from("m#lod0#surface0"),
            vertices: Arc::new(vec![
                vertex([-20.0, 0.0, -20.0]),
                vertex([20.0, 0.0, -20.0]),
                vertex([20.0, 0.0, 20.0]),
                vertex([-20.0, 0.0, 20.0]),
            ]),
            indices: Arc::new(vec![0, 1, 2, 0, 2, 3]),
            bones: Arc::new(vec![bone]),
            jiggle_offsets: [[0.0; 4]; 4],
            axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            origin: [0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn skinning_matches_identity_pose_plus_entity_origin() {
        let mut snap = quad();
        snap.origin = [10.0, 20.0, 30.0];
        let (position, normal) = skin_vertex(&snap.vertices[2], &snap);
        assert_eq!(position, [30.0, 20.0, 50.0]);
        assert_eq!(normal, [0.0, -1.0, 0.0]);
    }

    #[test]
    fn a_mark_covers_nearby_triangles_from_the_shooters_side_only() {
        let snap = quad();
        // Shot travelling +Y hits the front (-Y) face at the centre.
        let gore = build_gore(&[snap.clone()], 7, [0.0, -5.0, 0.0], [0.0, 5.0, 0.0], false, 8.0);
        assert_eq!(gore.len(), 1);
        assert_eq!(&*gore[0].key, "m#lod0#surface0#gore7");
        assert_eq!(gore[0].indices.len(), 6);
        // UVs centre on the hit: the quad corners lie outside 0..1.
        let centre_uv = gore[0].vertices[0].uv;
        assert!(centre_uv[0] < 0.0 || centre_uv[0] > 1.0);
        // From behind the same quad is back-facing: no mark.
        assert!(build_gore(&[snap.clone()], 8, [0.0, 5.0, 0.0], [0.0, -5.0, 0.0], false, 8.0).is_empty());
        // A special trace that misses the mesh places nothing.
        assert!(build_gore(&[snap.clone()], 9, [100.0, -5.0, 0.0], [100.0, 5.0, 0.0], true, 8.0).is_empty());
        // ...and one that hits it centres the mark on the surface.
        assert_eq!(build_gore(&[snap], 10, [0.0, -5.0, 0.0], [0.0, 5.0, 0.0], true, 8.0).len(), 1);
    }
}
