use super::cloth::{ClothCapsule, ClothRestCapsule, ClothSystem};
use jka_assets::ghoul2::{multiply_3x4, GlaAnimation, GlmModel, Matrix3x4};
use std::collections::HashMap;

fn fit_body_ellipsoid(points: &[[f32; 3]]) -> Option<ClothCapsule> {
    let points = points
        .iter()
        .copied()
        .map(glam::Vec3::from_array)
        .filter(|p| p.is_finite())
        .collect::<Vec<_>>();
    if points.is_empty() {
        return None;
    }
    let min = points
        .iter()
        .copied()
        .fold(glam::Vec3::splat(f32::INFINITY), glam::Vec3::min);
    let max = points
        .iter()
        .copied()
        .fold(glam::Vec3::splat(f32::NEG_INFINITY), glam::Vec3::max);
    let center = (min + max) * 0.5;
    let half = ((max - min) * 0.5).max(glam::Vec3::splat(0.25));
    let fit = points
        .iter()
        .map(|p| ((*p - center) / half).length())
        .fold(1.0_f32, f32::max);
    let radii = half * fit;
    Some(ClothCapsule {
        a: center.to_array(),
        b: center.to_array(),
        radius: radii.max_element(),
        basis: Some(glam::Mat3::from_diagonal(radii).to_cols_array_2d()),
        rest: None,
    })
}

/// Same cached body geometry for the renderer and offline cloth reproduction.
pub(crate) fn build_body_templates(
    glm: &GlmModel,
    gla: &GlaAnimation,
    visible: &[usize],
    lod_index: usize,
) -> Vec<(usize, ClothCapsule)> {
    // Points keep their dominant weight: blended vertices (skirt flaps, pads
    // or capes skinned partly to a limb) must not inflate that limb's proxy.
    let mut clouds = HashMap::<usize, Vec<([f32; 3], f32)>>::new();
    if let Some(lod) = glm.lods.get(lod_index) {
        for surface in &lod.surfaces {
            if !visible.contains(&surface.surface_index) {
                continue;
            }
            let name = glm
                .hierarchy
                .get(surface.surface_index)
                .map(|entry| entry.name.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ClothSystem::is_cloth_surface_name(&name)
                || name.contains("robe")
                || name.contains("cape")
                || name.contains("cloak")
                || name.starts_with("tag_")
                || name.starts_with("*")
                || name.contains("_cap")
            {
                continue;
            }
            for vertex in &surface.vertices {
                let Some(weight) = vertex
                    .weights
                    .iter()
                    .max_by(|a, b| a.weight.total_cmp(&b.weight))
                else {
                    continue;
                };
                let Some(&bone) = surface.bone_references.get(weight.local_bone_index) else {
                    continue;
                };
                if bone < gla.skeleton.len() {
                    clouds
                        .entry(bone)
                        .or_default()
                        .push((vertex.position, weight.weight));
                }
            }
        }
    }
    let mut templates = Vec::new();
    let mut bones = clouds.keys().copied().collect::<Vec<_>>();
    bones.sort_unstable();
    for bone in bones {
        let points = &clouds[&bone];
        if points.len() < 4 {
            continue;
        }
        // Limb/spine bones with a known child joint get a capsule along the
        // bone segment. A bind-space bounding ellipsoid of every dominant
        // vertex is badly inflated by attached clothing (a skirt flap skinned
        // to the shin made a 14-unit "shin" that kicked capes while running).
        if let Some(capsule) = fit_bone_segment_capsule(gla, bone, points) {
            templates.push((bone, capsule));
            continue;
        }
        let points = points.iter().map(|(p, _)| *p).collect::<Vec<_>>();
        if let Some(capsule) = fit_body_ellipsoid(&points) {
            templates.push((bone, capsule));
        }
    }
    templates
}

/// Humanoid bone segments (start, end, fallback radius in JKA units).
/// Entries with a fallback are also used when a bone has no visible
/// geometry (limbs hidden under a robe). Twist bones are measured only.
const BODY_SEGMENTS: &[(&str, &str, Option<f32>)] = &[
    ("pelvis", "lower_lumbar", Some(7.0)),
    ("lower_lumbar", "upper_lumbar", Some(7.0)),
    ("upper_lumbar", "thoracic", Some(6.5)),
    ("thoracic", "cervical", Some(5.5)),
    ("cranium", "cranium", Some(5.5)),
    ("rhumerus", "rradius", Some(3.2)),
    ("rhumerusX", "rradius", None),
    ("rradius", "rhand", Some(2.8)),
    ("rradiusX", "rhand", None),
    ("lhumerus", "lradius", Some(3.2)),
    ("lhumerusX", "lradius", None),
    ("lradius", "lhand", Some(2.8)),
    ("lradiusX", "lhand", None),
    ("rfemurYZ", "rtibia", Some(4.2)),
    ("rfemurX", "rtibia", None),
    ("lfemurYZ", "ltibia", Some(4.2)),
    ("lfemurX", "ltibia", None),
    ("rtibia", "rtalus", Some(3.4)),
    ("ltibia", "ltalus", Some(3.4)),
];

fn bone_index(gla: &GlaAnimation, name: &str) -> Option<usize> {
    gla.skeleton
        .iter()
        .position(|bone| bone.name.eq_ignore_ascii_case(name))
}

fn bind_origin(gla: &GlaAnimation, index: usize) -> Option<[f32; 3]> {
    let matrix = gla.skeleton.get(index)?.base_pose;
    Some([matrix[0][3], matrix[1][3], matrix[2][3]])
}

/// Capsule along a bone's bind segment. Radius is a high percentile of the
/// distances of mostly-rigid vertices, so a minority of blended or outlying
/// clothing vertices cannot inflate it. Bones without a distinct child joint
/// (head, hands, feet, clavicles) keep the ellipsoid fit.
fn fit_bone_segment_capsule(
    gla: &GlaAnimation,
    bone: usize,
    points: &[([f32; 3], f32)],
) -> Option<ClothCapsule> {
    const RIGID_WEIGHT: f32 = 0.6;
    const PERCENTILE: f32 = 0.9;
    let name = &gla.skeleton.get(bone)?.name;
    let &(_, end, fallback) = BODY_SEGMENTS
        .iter()
        .find(|(start, end, _)| start.eq_ignore_ascii_case(name) && !start.eq_ignore_ascii_case(end))?;
    let a = glam::Vec3::from_array(bind_origin(gla, bone)?);
    let b = glam::Vec3::from_array(bind_origin(gla, bone_index(gla, end)?)?);
    if a.distance_squared(b) < 1.0e-4 {
        return None;
    }
    let rigid = points
        .iter()
        .filter(|(_, weight)| *weight >= RIGID_WEIGHT)
        .count();
    let mut distances = points
        .iter()
        .filter(|(_, weight)| rigid < 6 || *weight >= RIGID_WEIGHT)
        .map(|(p, _)| {
            let p = glam::Vec3::from_array(*p);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            p.distance(a + ab * t)
        })
        .filter(|d| d.is_finite())
        .collect::<Vec<_>>();
    if distances.len() < 4 {
        return None;
    }
    distances.sort_by(f32::total_cmp);
    let index = ((distances.len() - 1) as f32 * PERCENTILE).round() as usize;
    // Never thinner than half the stock rig nor thicker than twice it.
    let reference = fallback.unwrap_or(3.0);
    let radius = distances[index].clamp(reference * 0.5, reference * 2.0);
    Some(ClothCapsule {
        a: a.to_array(),
        b: b.to_array(),
        radius,
        basis: None,
        rest: None,
    })
}
pub(crate) fn pose_body_capsules(
    templates: &[(usize, ClothCapsule)],
    gla: &GlaAnimation,
    pose: &[Matrix3x4],
) -> Vec<ClothCapsule> {
    let transform = |bone: usize, capsule: &ClothCapsule| -> Option<ClothCapsule> {
        let matrix = pose.get(bone)?;
        let point = |p: [f32; 3]| {
            std::array::from_fn(|row| {
                matrix[row][0] * p[0]
                    + matrix[row][1] * p[1]
                    + matrix[row][2] * p[2]
                    + matrix[row][3]
            })
        };
        let scale = (0..3)
            .map(|column| {
                (0..3)
                    .map(|row| matrix[row][column].powi(2))
                    .sum::<f32>()
                    .sqrt()
            })
            .fold(0.0_f32, f32::max);
        Some(ClothCapsule {
            a: point(capsule.a),
            b: point(capsule.b),
            radius: capsule.radius * scale,
            basis: capsule.basis.map(|axes| {
                std::array::from_fn(|column| {
                    std::array::from_fn(|row| {
                        matrix[row][0] * axes[column][0]
                            + matrix[row][1] * axes[column][1]
                            + matrix[row][2] * axes[column][2]
                    })
                })
            }),
            // Templates are fitted in bind space.
            rest: Some(ClothRestCapsule {
                a: capsule.a,
                b: capsule.b,
                radius: capsule.radius,
                basis: capsule.basis,
            }),
        })
    };
    let mut capsules = templates
        .iter()
        .filter_map(|(bone, capsule)| transform(*bone, capsule))
        .collect::<Vec<_>>();

    // Hidden limbs often have no body triangles under a robe. Keep the
    // skeletal support used by the existing humanoid collision rig there.
    let bone_origin = |index: usize| -> Option<[f32; 3]> {
        let matrix = multiply_3x4(pose.get(index)?, &gla.skeleton.get(index)?.base_pose);
        Some([matrix[0][3], matrix[1][3], matrix[2][3]])
    };
    for &(start, end, radius) in BODY_SEGMENTS {
        let Some(radius) = radius else {
            continue;
        };
        let Some(start) = bone_index(gla, start) else {
            continue;
        };
        let Some(end) = bone_index(gla, end) else {
            continue;
        };
        // A measured proxy attached to this bone takes precedence.
        if templates.iter().any(|(bone, _)| *bone == start) {
            continue;
        }
        if let (Some(a), Some(b)) = (bone_origin(start), bone_origin(end)) {
            capsules.push(ClothCapsule {
                a,
                b,
                radius,
                basis: None,
                rest: match (bind_origin(gla, start), bind_origin(gla, end)) {
                    (Some(a), Some(b)) => Some(ClothRestCapsule {
                        a,
                        b,
                        radius,
                        basis: None,
                    }),
                    _ => None,
                },
            });
        }
    }
    capsules
}
