use super::cloth::{ClothCapsule, ClothSystem};
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
    })
}

/// Same cached body geometry for the renderer and offline cloth reproduction.
pub(crate) fn build_body_templates(
    glm: &GlmModel,
    gla: &GlaAnimation,
    visible: &[usize],
    lod_index: usize,
) -> Vec<(usize, ClothCapsule)> {
    let mut clouds = HashMap::<usize, Vec<[f32; 3]>>::new();
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
                    clouds.entry(bone).or_default().push(vertex.position);
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
        if let Some(capsule) = fit_body_ellipsoid(points) {
            templates.push((bone, capsule));
        }
    }
    templates
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
    for (start, end, radius) in [
        ("pelvis", "lower_lumbar", 7.0),
        ("lower_lumbar", "upper_lumbar", 7.0),
        ("upper_lumbar", "thoracic", 6.5),
        ("thoracic", "cervical", 5.5),
        ("cranium", "cranium", 5.5),
        ("rhumerus", "rradius", 3.2),
        ("rradius", "rhand", 2.8),
        ("lhumerus", "lradius", 3.2),
        ("lradius", "lhand", 2.8),
        ("rfemurYZ", "rtibia", 4.2),
        ("lfemurYZ", "ltibia", 4.2),
        ("rtibia", "rtalus", 3.4),
        ("ltibia", "ltalus", 3.4),
    ] {
        let Some(start) = gla
            .skeleton
            .iter()
            .position(|bone| bone.name.eq_ignore_ascii_case(start))
        else {
            continue;
        };
        let Some(end) = gla
            .skeleton
            .iter()
            .position(|bone| bone.name.eq_ignore_ascii_case(end))
        else {
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
            });
        }
    }
    capsules
}
