//! Per-cut mesh work: CPU skin of the snapshot pose, vertex labelling, the
//! LimbHacker slice, cap triangulation and GPU mesh-source building. It is
//! queued on the app's shared, program-lifetime `jka-background-cpu` lane;
//! no thread is owned here.
//!
//! The result is pose independent (bind-space meshes, segment fractions,
//! body-local points), so the main thread can apply it a frame or two later
//! while the corpse keeps simulating as one piece in between.

use super::{
    add, build_cut_model, length, normalize, scale, slice, sub, transform_jka_model_point,
    Arc, HashMap, HashSet, Label, Matrix3x4, PlayerModelAsset, SkeletonCut, SliceSource, Swing,
    TargetKey, CAP_UV_SCALE, MAX_PIECES,
};
use crate::cgame::ragdoll::{ragdoll_driver_for_bone_by, rigidify_affine};
use jka_assets::ghoul2::{multiply_3x4, skin_glm_surface};
/// Body-local sample points used to size a synthesized Rapier body.
const SAMPLE_POINTS: usize = 96;

/// Identity and presentation data that travels with a job and comes back
/// with its result, so the main thread can reject stale results.
pub(super) struct CutMeta {
    pub key: TargetKey,
    pub ragdoll_key: u16,
    /// Rapier instance serial when the job was submitted.
    pub serial: u64,
    /// The mesh that was cut; the result is stale once the target changes.
    pub source_model: Arc<PlayerModelAsset>,
    pub client: i32,
    pub rgba: [f32; 4],
    pub rgb: [f32; 3],
    pub swing: Swing,
}

pub(super) struct CutJob {
    pub meta: CutMeta,
    pub pose: Vec<Matrix3x4>,
    pub axis: [[f32; 3]; 3],
    pub origin: [f32; 3],
    pub visible: HashSet<usize>,
    pub cut: SkeletonCut,
    pub cut_id: u64,
    pub cap_texture: Option<Arc<crate::materials::TextureData>>,
}

pub(super) struct ComputedPiece {
    pub model: Arc<PlayerModelAsset>,
    pub bones: Vec<usize>,
    pub synthesize: Option<(usize, Vec<[f32; 3]>)>,
    pub kick: [f32; 3],
    pub anchor_bone: usize,
}

pub(super) struct ComputedCut {
    pub root: Arc<PlayerModelAsset>,
    pub pieces: Vec<ComputedPiece>,
    pub truncate: Vec<(usize, f32)>,
    /// World-space (origin, outward dir, is root face) per new cap.
    pub faces: Vec<([f32; 3], [f32; 3], bool)>,
    pub hand_detached: bool,
    pub crossings: usize,
}

pub(super) struct CutDone {
    pub meta: CutMeta,
    pub outcome: Result<ComputedCut, String>,
}

/// Runs on the shared `jka-background-cpu` lane (or inline without one).
pub(super) fn run(job: CutJob) -> CutDone {
    let CutJob { meta, pose, axis, origin, visible, cut, cut_id, cap_texture } = job;
    let outcome = compute_cut(&meta, &pose, axis, origin, &visible, &cut, cut_id, cap_texture);
    CutDone { meta, outcome }
}

#[allow(clippy::too_many_arguments)]
fn compute_cut(
    meta: &CutMeta,
    pose: &[Matrix3x4],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    visible: &HashSet<usize>,
    cut: &SkeletonCut,
    cut_id: u64,
    cap_texture: Option<Arc<crate::materials::TextureData>>,
) -> Result<ComputedCut, String> {
    let model = &meta.source_model;
    let gla = &model.gla;
    let lod = model.glm.lods.first().ok_or("cut model has no LOD")?;
    let model_scale = length(axis[0]).max(1.0e-3);

    // Vertex -> driving segment, exactly as the ragdoll drives bones.
    let segment_of_bone: HashMap<usize, usize> =
        cut.segments.iter().enumerate().map(|(i, s)| (s.bone, i)).collect();
    let driver: Vec<Option<usize>> = (0..gla.skeleton.len())
        .map(|bone| {
            ragdoll_driver_for_bone_by(gla, bone, |b| segment_of_bone.contains_key(&b))
                .and_then(|b| segment_of_bone.get(&b).copied())
        })
        .collect();

    let mut group_label = HashMap::<usize, Label>::new();
    group_label.insert(cut.root_group, 0);

    struct Mandatory {
        segment: usize,
        point: [f32; 3],
        reach: f32,
        neighbours: Vec<usize>,
    }
    let mandatory: Vec<Mandatory> = cut
        .crossings
        .iter()
        .map(|c| {
            let mut neighbours: Vec<usize> = cut.segments[c.segment].parent.into_iter().collect();
            neighbours.extend((0..cut.segments.len()).filter(|&i| cut.segments[i].parent == Some(c.segment)));
            Mandatory {
                segment: c.segment,
                point: c.model_point,
                reach: cut.segments[c.segment].radius * 2.5 + 4.0,
                neighbours,
            }
        })
        .collect();

    let mut sources = Vec::new();
    for surface in lod.surfaces.iter().filter(|s| visible.contains(&s.surface_index)) {
        let Ok(skinned) = skin_glm_surface(surface, pose) else { continue };
        let posed: Vec<[f32; 3]> = skinned.vertices.iter().map(|v| v.position).collect();
        if posed.len() != surface.vertices.len() {
            continue;
        }
        let mut labels = Vec::with_capacity(posed.len());
        for (vertex, &p) in surface.vertices.iter().zip(&posed) {
            let mut by_segment: Vec<(usize, f32)> = Vec::with_capacity(4);
            for w in &vertex.weights {
                let Some(&bone) = surface.bone_references.get(w.local_bone_index) else { continue };
                let Some(&Some(seg)) = driver.get(bone) else { continue };
                match by_segment.iter_mut().find(|entry| entry.0 == seg) {
                    Some(entry) => entry.1 += w.weight,
                    None => by_segment.push((seg, w.weight)),
                }
            }
            let crossed = by_segment
                .iter()
                .filter(|(s, w)| *w > 0.1 && cut.crossed(*s))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(s, _)| *s)
                .or_else(|| {
                    // LimbHacker "mandatory" bones: near the cut, vertices on
                    // the crossed segment's parent/children are plane tested
                    // too, so the piece boundary lies on the plane (where
                    // caps are generated) instead of along triangle edges.
                    mandatory
                        .iter()
                        .filter(|m| {
                            by_segment.iter().any(|(s, w)| *w > 0.1 && m.neighbours.contains(s))
                                && length(sub(p, m.point)) <= m.reach
                        })
                        .min_by(|a, b| length(sub(p, a.point)).total_cmp(&length(sub(p, b.point))))
                        .map(|m| m.segment)
                });
            let group = if let Some(seg) = crossed {
                let same = (super::side(&cut.plane_model, p) >= 0.0)
                    == (super::side(&cut.plane_model, cut.segments[seg].start) >= 0.0);
                if same { cut.near(seg) } else { cut.far(seg) }
            } else if let Some((seg, _)) = by_segment.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
                cut.near(*seg)
            } else {
                cut.root_group
            };
            let next = group_label.len();
            let label = if next >= MAX_PIECES && !group_label.contains_key(&group) {
                0
            } else {
                *group_label.entry(group).or_insert(next as Label)
            };
            labels.push(label);
        }
        sources.push(SliceSource { surface, posed, labels });
    }
    let label_count = group_label.len();
    if label_count < 2 {
        return Err("no vertices on a severed side".to_owned());
    }
    let mut groups_by_label = vec![0; label_count];
    for (&group, &label) in &group_label {
        groups_by_label[usize::from(label)] = group;
    }

    let output = slice::slice(&sources, cut.plane_model, label_count, pose, CAP_UV_SCALE);
    if output.pieces.iter().filter(|p| !p.is_empty()).count() < 2 || output.pieces[0].is_empty() {
        return Err("degenerate split".to_owned());
    }

    let sweep = meta.swing.sweep_direction();
    let root_model_centre = (0..cut.segments.len())
        .find(|&i| cut.near(i) == cut.root_group)
        .map_or([0.0; 3], |i| cut.segments[i].start);
    let root_centre = transform_jka_model_point(root_model_centre, axis, origin);

    let mut pieces_out = output.pieces;
    let mut caps_out = output.caps;
    let root = build_cut_model(model, std::mem::take(&mut pieces_out[0]), caps_out[0].take(), cap_texture.clone(), cut_id, 0)?;
    let mut pieces = Vec::new();
    for label in 1..label_count {
        if pieces_out[label].is_empty() {
            continue;
        }
        let group = groups_by_label[label];
        let mut members: Vec<usize> = (0..cut.segments.len()).filter(|&i| cut.near(i) == group).collect();
        // Anchor first: the member whose parent is outside the piece.
        members.sort_by_key(|&i| cut.segments[i].parent.is_some_and(|p| cut.near(p) == group));
        let bones: Vec<usize> = members.iter().map(|&i| cut.segments[i].bone).collect();
        let points = sample_model_points(&sources, label as Label);
        let synthesize = if bones.is_empty() {
            cut.crossings.iter().find(|c| cut.far(c.segment) == group).map(|c| {
                let bone = cut.segments[c.segment].bone;
                (bone, body_local_points(gla, pose, bone, &points, model_scale))
            })
        } else {
            None
        };
        if bones.is_empty() && synthesize.is_none() {
            continue;
        }
        let anchor_bone = bones.first().copied().or_else(|| synthesize.as_ref().map(|s| s.0)).unwrap_or(0);
        let centre = scale(points.iter().fold([0.0; 3], |a, &p| add(a, p)), 1.0 / points.len().max(1) as f32);
        let outward = normalize(sub(transform_jka_model_point(centre, axis, origin), root_centre));
        let kick = add(add(scale(sweep, 140.0), scale(outward, 40.0)), [0.0, 0.0, 30.0]);
        match build_cut_model(model, std::mem::take(&mut pieces_out[label]), caps_out[label].take(), cap_texture.clone(), cut_id, label) {
            Ok(model) => pieces.push(ComputedPiece { model, bones, synthesize, kick, anchor_bone }),
            Err(error) => { devprintln!(1, "DYNAMIC CUT: piece mesh failed: {error}"); }
        }
    }
    if pieces.is_empty() {
        return Err("no physical piece".to_owned());
    }

    let truncate = cut.crossings.iter().map(|c| (cut.segments[c.segment].bone, c.fraction)).collect();
    let faces = output
        .faces
        .iter()
        .take(8)
        .map(|face| {
            let n = face.normal;
            let dir = normalize([
                axis[0][0] * n[0] + axis[1][0] * n[1] + axis[2][0] * n[2],
                axis[0][1] * n[0] + axis[1][1] * n[1] + axis[2][1] * n[2],
                axis[0][2] * n[0] + axis[1][2] * n[1] + axis[2][2] * n[2],
            ]);
            (transform_jka_model_point(face.centroid, axis, origin), dir, face.label == 0)
        })
        .collect();
    let hand_detached = gla
        .skeleton
        .iter()
        .position(|bone| bone.name.eq_ignore_ascii_case("rhand"))
        .and_then(|bone| driver.get(bone).copied().flatten())
        .is_some_and(|seg| group_label.get(&cut.near(seg)).is_some_and(|&label| label != 0));
    Ok(ComputedCut { root, pieces, truncate, faces, hand_detached, crossings: cut.crossings.len() })
}

/// Up to `SAMPLE_POINTS` posed model-space points of one label.
fn sample_model_points(sources: &[SliceSource<'_>], label: Label) -> Vec<[f32; 3]> {
    let total: usize = sources.iter().map(|s| s.labels.iter().filter(|&&l| l == label).count()).sum();
    let stride = (total / SAMPLE_POINTS).max(1);
    let mut out = Vec::with_capacity(SAMPLE_POINTS + 1);
    let mut k = 0usize;
    for source in sources {
        for (p, &l) in source.posed.iter().zip(&source.labels) {
            if l != label {
                continue;
            }
            if k % stride == 0 {
                out.push(*p);
            }
            k += 1;
        }
    }
    out
}

/// Model-space points expressed in the Rapier body frame of `bone`: the
/// rigidified posed bone with its translation scaled by modelScale, exactly
/// how ragdoll bodies are placed, so this stays valid while the corpse moves.
fn body_local_points(
    gla: &jka_assets::ghoul2::GlaAnimation,
    pose: &[Matrix3x4],
    bone: usize,
    points: &[[f32; 3]],
    model_scale: f32,
) -> Vec<[f32; 3]> {
    let (Some(skin), Some(base)) = (pose.get(bone), gla.skeleton.get(bone)) else {
        return Vec::new();
    };
    let m = rigidify_affine(&multiply_3x4(skin, &base.base_pose));
    let t = [m[0][3], m[1][3], m[2][3]];
    points
        .iter()
        .map(|&p| {
            let d = sub(p, t);
            // R^T * d (columns of the rigid rotation are the bone axes).
            scale([
                m[0][0] * d[0] + m[1][0] * d[1] + m[2][0] * d[2],
                m[0][1] * d[0] + m[1][1] * d[1] + m[2][1] * d[2],
                m[0][2] * d[0] + m[1][2] * d[1] + m[2][2] * d[2],
            ], model_scale)
        })
        .collect()
}
