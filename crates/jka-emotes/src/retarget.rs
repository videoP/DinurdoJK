//! Rest-relative affine retargeting with target proportions and animated scale.
//! This preserves target proportions, but does not solve contact IK or props.
use crate::psa::Psa;
use glam::{Mat3, Quat, Vec3};
use jka_assets::ghoul2::{GlaAnimation, Matrix3x4};
use serde_json::Value;
use std::collections::HashMap;

fn position(m: &Matrix3x4) -> Vec3 {
    Vec3::new(m[0][3], m[1][3], m[2][3])
}
fn mapped_source(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "pelvis" => "pelvis",
        "lower_lumbar" => "spine_01",
        "upper_lumbar" => "spine_02",
        "thoracic" => "spine_05",
        "cervical" => "neck_02",
        "cranium" => "head",
        "lfemuryz" => "thigh_l",
        "ltibia" => "calf_l",
        "ltalus" => "foot_l",
        "ltarsal" => "ball_l",
        "rfemuryz" => "thigh_r",
        "rtibia" => "calf_r",
        "rtalus" => "foot_r",
        "rtarsal" => "ball_r",
        "lclavical" => "clavicle_l",
        "lhumerus" => "upperarm_l",
        "lradius" => "lowerarm_l",
        "lhand" => "hand_l",
        "rclavical" => "clavicle_r",
        "rhumerus" => "upperarm_r",
        "rradius" => "lowerarm_r",
        "rhand" => "hand_r",
        // X helpers retain their rest-relative transform under the animated limb.
        "l_d1_j1" => "thumb_01_l",
        "l_d1_j2" => "thumb_02_l",
        "l_d1_j3" => "thumb_03_l",
        "l_d2_j1" => "index_01_l",
        "l_d2_j2" => "index_02_l",
        "l_d2_j3" => "index_03_l",
        "l_d3_j1" => "middle_01_l",
        "l_d3_j2" => "middle_02_l",
        "l_d3_j3" => "middle_03_l",
        "l_d4_j1" => "ring_01_l",
        "l_d4_j2" => "ring_02_l",
        "l_d4_j3" => "ring_03_l",
        "l_d5_j1" => "pinky_01_l",
        "l_d5_j2" => "pinky_02_l",
        "l_d5_j3" => "pinky_03_l",
        "r_d1_j1" => "thumb_01_r",
        "r_d1_j2" => "thumb_02_r",
        "r_d1_j3" => "thumb_03_r",
        "r_d2_j1" => "index_01_r",
        "r_d2_j2" => "index_02_r",
        "r_d2_j3" => "index_03_r",
        "r_d3_j1" => "middle_01_r",
        "r_d3_j2" => "middle_02_r",
        "r_d3_j3" => "middle_03_r",
        "r_d4_j1" => "ring_01_r",
        "r_d4_j2" => "ring_02_r",
        "r_d4_j3" => "ring_03_r",
        "r_d5_j1" => "pinky_01_r",
        "r_d5_j2" => "pinky_02_r",
        "r_d5_j3" => "pinky_03_r",
        _ => return None,
    })
}

pub fn bake(
    psa: &Psa,
    target: &GlaAnimation,
    settings: &Value,
) -> Result<(Vec<u8>, Vec<u8>, Value), String> {
    use glam::{Mat4, Vec4};
    target.compose_frame(0)?;
    // Native humanoid clips translate model_root down from the controller
    // origin. Bind matrices omit this translation, leaving players afloat.
    let root = target
        .skeleton
        .iter()
        .position(|b| b.parent < 0)
        .ok_or("target has no root")?;
    let origin_z = settings
        .get("origin_offset_z")
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .unwrap_or(target.decompress_bone(0, root)?[2][3]);
    if !origin_z.is_finite() || origin_z.abs() > 256.0 {
        return Err("invalid origin_offset_z".into());
    }
    let source_index: HashMap<_, _> = psa
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.to_ascii_lowercase(), i))
        .collect();
    let custom = settings.get("bone_map").and_then(Value::as_object);
    if let Some(custom) = custom {
        for name in custom.keys() {
            if !target.skeleton.iter().any(|b| b.name == *name) {
                return Err(format!("unknown target bone in bone_map: {name}"));
            }
        }
    }
    let mut mapping = Vec::new();
    let mut report = serde_json::Map::new();
    for b in &target.skeleton {
        let source = if let Some(name) = custom.and_then(|m| m.get(&b.name)) {
            let name = name
                .as_str()
                .ok_or("bone_map values must be source bone names")?;
            Some(
                *source_index
                    .get(&name.to_ascii_lowercase())
                    .ok_or_else(|| format!("missing source bone {name}"))?,
            )
        } else {
            mapped_source(&b.name)
                .and_then(|name| source_index.get(name).copied())
                .or_else(|| {
                    // Extracted Bean/Minifig rigs use the same anatomical roles
                    // with shorter chains. Missing elbows/knees inherit the
                    // parent skin transform rather than inventing joint motion.
                    let alternatives: &[&str] = match b.name.to_ascii_lowercase().as_str() {
                        "lower_lumbar" | "upper_lumbar" => &["torso"],
                        "thoracic" => &["spine_02", "torso"],
                        "cranium" => &["head_01"],
                        "cervical" => &["neck_01", "neck_extension"],
                        "lfemuryz" => &["leg_l"],
                        "rfemuryz" => &["leg_r"],
                        "lhumerus" => &["arm_l"],
                        "rhumerus" => &["arm_r"],
                        _ => &[],
                    };
                    alternatives
                        .iter()
                        .find_map(|name| source_index.get(*name).copied())
                })
        };
        if let Some(i) = source {
            report.insert(b.name.clone(), Value::String(psa.bones[i].name.clone()));
        }
        mapping.push(source);
    }
    for required in [
        "pelvis", "thoracic", "cranium", "lfemurYZ", "rfemurYZ", "lhumerus", "rhumerus",
    ] {
        let i = target
            .skeleton
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(required))
            .ok_or_else(|| format!("target is not a JKA humanoid: missing {required}"))?;
        if mapping[i].is_none() {
            return Err(format!("unmapped required bone {required}"));
        }
    }
    let source_rest = psa.world_rotations(None);
    let mut source_positions = Vec::new();
    for b in &psa.bones {
        source_positions.push(b.parent.map_or(b.rest_position, |p| {
            source_positions[p] + source_rest[p] * b.rest_position
        }));
    }
    let ti = |name: &str| {
        target
            .skeleton
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("missing target landmark {name}"))
    };
    let landmark = |name: &str| {
        mapping[ti(name)?].ok_or_else(|| format!("missing source landmark for {name}"))
    };
    let source_up = source_positions[landmark("cranium")?] - source_positions[landmark("pelvis")?];
    let target_up = position(&target.skeleton[ti("cranium")?].base_pose)
        - position(&target.skeleton[ti("pelvis")?].base_pose);
    let source_left =
        source_positions[landmark("lfemurYZ")?] - source_positions[landmark("rfemurYZ")?];
    let target_left = position(&target.skeleton[ti("lfemurYZ")?].base_pose)
        - position(&target.skeleton[ti("rfemurYZ")?].base_pose);
    let anatomical_frame = |left: Vec3, up: Vec3| -> Result<Mat3, String> {
        if up.length_squared() < 0.001 || left.cross(up).length_squared() < 0.001 {
            return Err("degenerate anatomical reference frame".into());
        }
        let up = up.normalize();
        let left = (left - up * up.dot(left)).normalize();
        Ok(Mat3::from_cols(left.cross(up).normalize(), left, up))
    };
    let basis = if let Some(values) = settings.get("basis_xyzw") {
        let a = values
            .as_array()
            .filter(|a| a.len() == 4)
            .ok_or("basis_xyzw needs four numbers")?;
        let mut q = [0.0; 4];
        for i in 0..4 {
            q[i] = a[i].as_f64().ok_or("invalid basis quaternion")? as f32;
        }
        let q = Quat::from_array(q);
        if !q.is_finite() || q.length_squared() < 0.001 {
            return Err("invalid basis quaternion".into());
        }
        q.normalize()
    } else {
        Quat::from_mat3(
            &(anatomical_frame(target_left, target_up)?
                * anatomical_frame(source_left, source_up)?.transpose()),
        )
        .normalize()
    };
    let scale = settings
        .get("translation_scale")
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .unwrap_or(target_up.length() / source_up.length());
    if !scale.is_finite() || scale <= 0.0 {
        return Err("translation_scale must be positive".into());
    }
    let start = settings
        .get("start_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let end = settings
        .get("end_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(psa.frames as f64 / psa.fps as f64);
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end <= start
        || end > psa.frames as f64 / psa.fps as f64 + 0.0001
    {
        return Err("invalid PSA trim interval".into());
    }
    let first = (start * psa.fps as f64).round() as usize;
    let last = (end * psa.fps as f64).round() as usize;
    if last <= first || last > psa.frames {
        return Err("PSA trim contains no complete samples".into());
    }
    if (last - first)
        .checked_mul(target.skeleton.len())
        .and_then(|n| n.checked_mul(48))
        .is_none_or(|n| n > crate::MAX_MEMBER_BYTES - 16)
    {
        return Err("baked pose track would exceed the 128 MiB member limit".into());
    }
    let matrix4 = |m: &Matrix3x4| {
        Mat4::from_cols(
            Vec4::new(m[0][0], m[1][0], m[2][0], 0.0),
            Vec4::new(m[0][1], m[1][1], m[2][1], 0.0),
            Vec4::new(m[0][2], m[1][2], m[2][2], 0.0),
            Vec4::new(m[0][3], m[1][3], m[2][3], 1.0),
        )
    };
    let bind: Vec<_> = target
        .skeleton
        .iter()
        .map(|b| matrix4(&b.base_pose))
        .collect();
    let inverse: Vec<_> = target
        .skeleton
        .iter()
        .map(|b| matrix4(&b.base_pose_inv))
        .collect();
    let basis_matrix = Mat3::from_quat(basis);
    // Finger rotations live in each phalanx's anatomical frame, rather than the
    // body frame. UE's bent reference fingers and JKA's straight fingers have
    // different hinge axes. Transfer the motion relative to the mapped parent
    // and align the longitudinal/palm axes before applying it.
    let mut fingers = vec![None; target.skeleton.len()];
    let extension_limit = settings["finger_extension_degrees"]
        .as_f64()
        .unwrap_or(10.0) as f32;
    if !extension_limit.is_finite() || !(0.0..=45.0).contains(&extension_limit) {
        return Err("finger_extension_degrees must be between 0 and 45".into());
    }
    let finger_frame = |direction: Vec3, width: Vec3| -> Option<Mat3> {
        if direction.length_squared() < 0.00001 {
            return None;
        }
        let x = direction.normalize();
        let y = width - x * width.dot(x);
        if y.length_squared() < 0.00001 {
            return None;
        }
        let y = y.normalize();
        Some(Mat3::from_cols(x, y, x.cross(y)))
    };
    for (i, b) in target.skeleton.iter().enumerate() {
        if !b.name.to_ascii_lowercase().contains("_d") || b.parent < 0 {
            continue;
        }
        let Some(source) = mapping[i] else {
            continue;
        };
        let Some(parent) = jka_assets::skeleton::mesh_pose_parent(&target.skeleton, i) else {
            continue;
        };
        let Some(ancestor) = mapping[parent] else {
            continue;
        };
        let side = if b.name.starts_with('l') { "l" } else { "r" };
        let target_width = ti(&format!("{side}_d2_j1"))
            .ok()
            .zip(
                ti(&format!("{side}_d5_j1"))
                    .or_else(|_| ti(&format!("{side}_d4_j1")))
                    .ok(),
            )
            .map(|(a, b)| {
                position(&target.skeleton[a].base_pose) - position(&target.skeleton[b].base_pose)
            });
        let source_width = source_index
            .get(&format!("index_01_{side}"))
            .zip(source_index.get(&format!("pinky_01_{side}")))
            .map(|(a, b)| source_positions[*a] - source_positions[*b]);
        let child = psa
            .bones
            .iter()
            .enumerate()
            .find(|(_, b)| {
                b.parent == Some(source)
                    && b.name
                        .starts_with(psa.bones[source].name.split('_').next().unwrap_or(""))
            })
            .map(|(n, _)| n);
        let source_direction = child
            .map(|c| source_positions[c] - source_positions[source])
            .or_else(|| {
                psa.bones[source]
                    .parent
                    .map(|p| source_positions[source] - source_positions[p])
            });
        let target_direction = target
            .skeleton
            .iter()
            .enumerate()
            .find(|(c, _)| jka_assets::skeleton::mesh_pose_parent(&target.skeleton, *c) == Some(i))
            .map(|(c, _)| c)
            .map(|c| position(&target.skeleton[c].base_pose) - position(&b.base_pose))
            .unwrap_or(position(&b.base_pose) - position(&target.skeleton[parent].base_pose));
        if let (Some(sw), Some(tw), Some(sd)) = (source_width, target_width, source_direction) {
            if let (Some(sf), Some(tf)) = (finger_frame(sd, sw), finger_frame(target_direction, tw))
            {
                let terminal = !b.name.ends_with("_j1");
                // UE's reference DIP/PIP chain is already curled. Subtracting
                // that rest curl from straight JKA distal fingers made an
                // authored open hand hyperextend backwards. Transfer its
                // anatomical curl, not only a delta from the curled reference.
                let curl = |before: Vec3, after: Vec3, width: Vec3| {
                    let a = before.normalize_or_zero();
                    let b = after.normalize_or_zero();
                    a.cross(b).dot(width).atan2(a.dot(b))
                };
                let rest_curl = if terminal {
                    curl(
                        source_positions[source] - source_positions[ancestor],
                        sd,
                        sf.y_axis,
                    )
                } else {
                    0.0
                };
                let target_curl = if terminal {
                    curl(
                        position(&b.base_pose) - position(&target.skeleton[parent].base_pose),
                        target_direction,
                        tf.y_axis,
                    )
                } else {
                    0.0
                };
                let hinge = if terminal {
                    Some((
                        tf.y_axis,
                        rest_curl - target_curl,
                        if rest_curl < 0.0 { -1.0 } else { 1.0 },
                        b.name.contains("_d1_"),
                    ))
                } else {
                    None
                };
                fingers[i] = Some((source, ancestor, tf * sf.transpose(), hinge));
            }
        }
    }
    let mut rigid_locals = Vec::new();
    let mut rigid_fallbacks = 0usize;
    let mut poses: Vec<Matrix3x4> = Vec::new();
    for frame in first..last {
        let mut source_linear = Vec::<Mat3>::with_capacity(psa.bones.len());
        for (i, b) in psa.bones.iter().enumerate() {
            let key = frame * psa.bones.len() + i;
            let local =
                Mat3::from_quat(psa.keys[key].rotation) * Mat3::from_diagonal(psa.scales[key]);
            source_linear.push(b.parent.map_or(local, |p| source_linear[p] * local));
        }
        let mut skin = vec![Mat4::IDENTITY; target.skeleton.len()];
        let rotations = psa.world_rotations(Some(frame));
        let finger_deltas: Vec<_> = fingers
            .iter()
            .map(|mapping| {
                mapping.map(|(s, p, alignment, hinge)| {
                    let parent = rotations[p] * source_rest[p].inverse();
                    let child = rotations[s] * source_rest[s].inverse();
                    let delta = alignment
                        * Mat3::from_quat(parent.inverse() * child)
                        * alignment.transpose();
                    if let Some((axis, offset, sign, thumb)) = hinge {
                        Mat3::from_quat(calibrated_finger_rotation(
                            Quat::from_mat3(&delta).normalize(),
                            axis,
                            offset,
                            sign,
                            if thumb {
                                35.0f32.to_radians()
                            } else {
                                extension_limit.to_radians()
                            },
                        ))
                    } else {
                        delta
                    }
                })
            })
            .collect();
        let mut done = vec![false; skin.len()];
        for i in 0..skin.len() {
            solve_skin(
                i,
                target,
                &mapping,
                &source_linear,
                &source_rest,
                &bind,
                &inverse,
                basis_matrix,
                &finger_deltas,
                &mut skin,
                &mut done,
                0,
            )?;
        }
        // Preserve hips/jumps; the root trajectory is owned by the game controller.
        let pelvis = ti("pelvis")?;
        let source = landmark("pelvis")?;
        let displacement =
            psa.keys[frame * psa.bones.len() + source].position - psa.bones[source].rest_position;
        let displacement = psa.bones[source]
            .parent
            .map_or(displacement, |p| source_rest[p] * displacement);
        let delta = basis * displacement * scale;
        for i in 0..skin.len() {
            // Motion is a bookkeeping helper; never turn its root trajectory into
            // a second movement owner. All real descendants receive hip motion.
            let mut ancestor = Some(i);
            let mut inherits = false;
            while let Some(n) = ancestor {
                if n == pelvis {
                    inherits = true;
                    break;
                }
                ancestor = usize::try_from(target.skeleton[n].parent).ok();
            }
            if inherits {
                skin[i].w_axis += delta.extend(0.0);
            }
            skin[i].w_axis.z += origin_z;
        }
        for i in 0..skin.len() {
            let m = skin[i];
            let cols = m.to_cols_array_2d();
            poses.push(std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r])));
            // GLA is the rig/rigid fallback. The game uses the lossless pose track
            // for PSA bundles, retaining scale and avoiding compression loss.
            let parent = target.skeleton[i].parent;
            let local = if parent < 0 {
                m
            } else if skin[parent as usize].determinant().abs() < 1e-8 {
                Mat4::IDENTITY
            } else {
                skin[parent as usize].inverse() * m
            };
            let (q, t) = if local.is_finite() && local.determinant().abs() >= 1e-8 {
                let (_, q, t) = local.to_scale_rotation_translation();
                (q, t)
            } else {
                (Quat::IDENTITY, Vec3::ZERO)
            };
            if !q.is_finite()
                || !t.is_finite()
                || t.abs().max_element() >= 511.98
                || m.determinant().abs() < 1e-8
                || (parent >= 0 && skin[parent as usize].determinant().abs() < 1e-8)
            {
                // The affine pose remains exact, including intentional collapse.
                // A singular transform has no rigid Ghoul2 representation.
                rigid_fallbacks += 1;
                rigid_locals.push((Quat::IDENTITY, Vec3::ZERO));
            } else {
                rigid_locals.push((q.normalize(), t));
            }
        }
    }
    // Follow source contacts, rather than lifting the whole clip to its single
    // lowest target foot sample. The source curve retains airborne intervals.
    let contact_heights = source_contact_heights(psa, basis, scale, first, last);
    let shifts = if settings["ground_contact"].as_bool().unwrap_or(true) {
        ground_shifts(
            &poses,
            target.num_bones(),
            &settings["ground_mesh"],
            origin_z,
            contact_heights.as_deref(),
        )?
    } else {
        vec![0.0; last - first]
    };
    for (frame, shift) in shifts.iter().enumerate() {
        for pose in &mut poses[frame * target.num_bones()..(frame + 1) * target.num_bones()] {
            pose[2][3] += shift;
        }
        for (i, (_, position)) in rigid_locals
            [frame * target.num_bones()..(frame + 1) * target.num_bones()]
            .iter_mut()
            .enumerate()
        {
            if target.skeleton[i].parent < 0 {
                position.z += shift;
            }
        }
    }
    let ground_shift = shifts.iter().copied().fold(f32::INFINITY, f32::min);
    let gla = encode(target, last - first, &rigid_locals)?;
    let parsed = jka_assets::ghoul2::parse_gla(&gla)?;
    crate::manifest::validate_gla(target, &parsed)?;
    let poses = crate::PoseClip::encode(last - first, target.skeleton.len(), &poses)?;
    Ok((
        gla,
        poses,
        serde_json::json!({"method":"rest-relative-affine-retarget", "bone_map":report,"source_frames":psa.frames,"fps":psa.fps,"first_source_frame":first,"end_source_frame":last,"basis_xyzw":basis.to_array(),"translation_scale":scale,"origin_offset_z":origin_z,"ground_shift_z":ground_shift,"ground_contact_method":if contact_heights.is_some(){"source-contact-curve"}else{"constant-fallback"},"root_motion":"stationary","scale":"preserved in baked pose track","finger_reference":"anatomical distal curl; per-hand signed hinge limits","finger_extension_degrees":extension_limit,"rigid_fallback_transforms":rigid_fallbacks,"limitations":["no contact IK","no props/VFX","verify facing and rest alignment visually"]}),
    ))
}

fn calibrated_finger_rotation(
    delta: Quat,
    axis: Vec3,
    offset: f32,
    sign: f32,
    extension: f32,
) -> Quat {
    let projected = delta.xyz().dot(axis);
    let twist = Quat::from_xyzw(
        axis.x * projected,
        axis.y * projected,
        axis.z * projected,
        delta.w,
    )
    .normalize();
    if !twist.is_finite() {
        return delta;
    }
    let angle = 2.0 * projected.atan2(delta.w);
    let angle =
        (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let corrected = ((angle + offset) * sign).clamp(-extension, 115.0f32.to_radians()) * sign;
    // Preserve off-axis articulation; only bound the anatomical curl component.
    ((delta * twist.inverse()) * Quat::from_axis_angle(axis, corrected)).normalize()
}

#[cfg(test)]
mod finger_tests {
    use super::*;
    #[test]
    fn open_fingers_do_not_subtract_the_curled_reference_on_either_hand() {
        for sign in [-1.0, 1.0] {
            let rest = sign * 35.0f32.to_radians();
            let open = calibrated_finger_rotation(
                Quat::from_axis_angle(Vec3::Y, -rest),
                Vec3::Y,
                rest,
                sign,
                10.0f32.to_radians(),
            );
            assert!(open.angle_between(Quat::IDENTITY) < 1e-5);
            let backwards = calibrated_finger_rotation(
                Quat::from_axis_angle(Vec3::Y, -rest - sign),
                Vec3::Y,
                rest,
                sign,
                10.0f32.to_radians(),
            );
            assert!(backwards.angle_between(Quat::IDENTITY) <= 10.01f32.to_radians());
            let closed = calibrated_finger_rotation(
                Quat::from_axis_angle(Vec3::Y, sign * 0.4),
                Vec3::Y,
                rest,
                sign,
                10.0f32.to_radians(),
            );
            assert!(closed.angle_between(Quat::IDENTITY) > 0.9);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn solve_skin(
    i: usize,
    target: &GlaAnimation,
    mapping: &[Option<usize>],
    source: &[Mat3],
    source_rest: &[Quat],
    bind: &[glam::Mat4],
    inverse: &[glam::Mat4],
    basis: Mat3,
    finger_deltas: &[Option<Mat3>],
    skin: &mut [glam::Mat4],
    done: &mut [bool],
    depth: usize,
) -> Result<(), String> {
    use glam::{Mat4, Vec4};
    if done[i] {
        return Ok(());
    }
    if depth > skin.len() {
        return Err("target skeleton cycle".into());
    }
    let parent =
        jka_assets::skeleton::mesh_pose_parent(&target.skeleton, i).map_or(-1, |p| p as i32);
    if parent >= 0 {
        solve_skin(
            parent as usize,
            target,
            mapping,
            source,
            source_rest,
            bind,
            inverse,
            basis,
            finger_deltas,
            skin,
            done,
            depth + 1,
        )?;
    }
    let parent_skin = if parent < 0 {
        Mat4::IDENTITY
    } else {
        skin[parent as usize]
    };
    let delta = if let Some(finger) = finger_deltas[i] {
        Mat3::from_mat4(parent_skin) * finger
    } else if let Some(s) = mapping[i] {
        basis * source[s] * Mat3::from_quat(source_rest[s]).transpose() * basis.transpose()
    } else {
        Mat3::from_mat4(parent_skin)
    };
    let linear = delta * Mat3::from_mat4(bind[i]);
    let p = parent_skin.transform_point3(bind[i].w_axis.truncate());
    let posed = Mat4::from_cols(
        linear.x_axis.extend(0.0),
        linear.y_axis.extend(0.0),
        linear.z_axis.extend(0.0),
        Vec4::new(p.x, p.y, p.z, 1.0),
    );
    skin[i] = posed * inverse[i];
    if !skin[i].is_finite() {
        return Err(format!(
            "non-finite retarget transform on {}",
            target.skeleton[i].name
        ));
    }
    done[i] = true;
    Ok(())
}

pub fn encode(
    target: &GlaAnimation,
    frames: usize,
    locals: &[(Quat, Vec3)],
) -> Result<Vec<u8>, String> {
    let bones = target.skeleton.len();
    if frames == 0 || locals.len() != frames.checked_mul(bones).ok_or("frame count overflow")? {
        return Err("invalid baked frame count".into());
    }
    let mut data = vec![0u8; 100 + bones * 4];
    data[..4].copy_from_slice(b"2LGA");
    put_u32(&mut data, 4, 6);
    data[8..19].copy_from_slice(b"djk_fnemote");
    put_u32(&mut data, 72, target.scale.to_bits());
    put_u32(&mut data, 76, frames as u32);
    put_u32(&mut data, 84, bones as u32);
    let skel_at = data.len();
    put_u32(&mut data, 92, skel_at as u32);
    for (i, b) in target.skeleton.iter().enumerate() {
        let at = data.len();
        put_u32(&mut data, 100 + i * 4, (at - 100) as u32);
        data.resize(at + 172, 0);
        let name = b.name.as_bytes();
        if name.len() >= 64 {
            return Err("target bone name too long".into());
        }
        data[at..at + name.len()].copy_from_slice(name);
        put_u32(&mut data, at + 64, b.flags);
        put_u32(&mut data, at + 68, b.parent as u32);
        for (n, m) in [b.base_pose, b.base_pose_inv].iter().enumerate() {
            for (r, row) in m.iter().enumerate() {
                for (c, value) in row.iter().enumerate() {
                    put_u32(
                        &mut data,
                        at + 72 + n * 48 + (r * 4 + c) * 4,
                        value.to_bits(),
                    );
                }
            }
        }
        put_u32(&mut data, at + 168, b.children.len() as u32);
        for &child in &b.children {
            data.extend_from_slice(&(child as u32).to_le_bytes());
        }
    }
    let frames_at = data.len();
    put_u32(&mut data, 80, frames_at as u32);
    let mut pool = Vec::new();
    let mut lookup = HashMap::new();
    for &(q, t) in locals {
        if !q.is_finite() || !t.is_finite() || t.abs().max_element() >= 511.98 {
            return Err("baked transform outside GLA compression range".into());
        }
        let mut packed = [0u8; 14];
        let q = q.normalize();
        // q and -q encode the same rotation. Canonical signs improve deduplication.
        let q = if q.w < 0.0 { -q } else { q };
        for (n, v) in [q.w, q.x, q.y, q.z].into_iter().enumerate() {
            packed[n * 2..n * 2 + 2]
                .copy_from_slice(&(((v + 2.0) * 16383.0).round() as u16).to_le_bytes());
        }
        for (n, v) in t.to_array().into_iter().enumerate() {
            packed[8 + n * 2..10 + n * 2]
                .copy_from_slice(&(((v + 512.0) * 64.0).round() as u16).to_le_bytes());
        }
        let next = pool.len() as u32;
        let index = *lookup.entry(packed).or_insert_with(|| {
            pool.push(packed);
            next
        });
        if index > 0xffffff {
            return Err("GLA compression pool exceeds 24-bit indices".into());
        }
        data.extend_from_slice(&index.to_le_bytes()[..3]);
    }
    let pool_at = data.len();
    put_u32(&mut data, 88, pool_at as u32);
    for packed in pool {
        data.extend_from_slice(&packed);
    }
    let end = data.len();
    put_u32(&mut data, 96, end as u32);
    Ok(data)
}
fn put_u32(data: &mut [u8], at: usize, n: u32) {
    data[at..at + 4].copy_from_slice(&n.to_le_bytes());
}

fn ground_shifts(
    poses: &[Matrix3x4],
    bones: usize,
    mesh: &Value,
    origin: f32,
    source_heights: Option<&[f32]>,
) -> Result<Vec<f32>, String> {
    if mesh.is_null() {
        return Ok(vec![0.0; poses.len() / bones]);
    }
    let bind_floor = mesh["bind_floor"]
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or("invalid ground reference floor")? as f32;
    let points = mesh["points"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 8192)
        .ok_or("invalid ground reference points")?;
    let mut markers = Vec::new();
    for point in points {
        let coordinates = point["position"]
            .as_array()
            .filter(|a| a.len() == 3)
            .ok_or("invalid ground marker position")?;
        let mut p = [0.0f32; 3];
        for i in 0..3 {
            p[i] = coordinates[i]
                .as_f64()
                .filter(|n| n.is_finite() && n.abs() < 10000.0)
                .ok_or("invalid ground marker position")? as f32;
        }
        let mut weights = Vec::new();
        for w in point["weights"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 4)
            .ok_or("invalid ground marker weights")?
        {
            let index = w[0]
                .as_u64()
                .filter(|n| *n < (bones as u64))
                .ok_or("invalid ground marker bone")? as usize;
            let weight = w[1]
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
                .ok_or("invalid ground marker weight")? as f32;
            weights.push((index, weight));
        }
        if (weights.iter().map(|(_, w)| w).sum::<f32>() - 1.0).abs() > 0.001 {
            return Err("ground marker weights must sum to one".into());
        }
        markers.push((p, weights));
    }
    let mut floors = Vec::new();
    for frame in poses.chunks_exact(bones) {
        let mut floor = f32::INFINITY;
        for (p, weights) in &markers {
            let z = weights
                .iter()
                .map(|(i, w)| {
                    let row = &frame[*i][2];
                    w * (row[0] * p[0] + row[1] * p[1] + row[2] * p[2] + row[3])
                })
                .sum::<f32>();
            floor = floor.min(z);
        }
        floors.push(floor);
    }
    let reference_floor = mesh["floor_z"]
        .as_f64()
        .filter(|n| n.is_finite() && n.abs() < 10000.0)
        .map(|n| n as f32)
        .unwrap_or(origin + bind_floor);
    let shifts = match source_heights {
        Some(heights) if heights.len() == floors.len() => floors
            .iter()
            .zip(heights)
            .map(|(floor, height)| reference_floor + height - floor)
            .collect::<Vec<_>>(),
        _ => vec![
            reference_floor - floors.iter().copied().fold(f32::INFINITY, f32::min);
            floors.len()
        ],
    };
    if shifts.iter().any(|s| !s.is_finite() || s.abs() > 256.0) {
        return Err("ground calibration outside supported range".into());
    }
    Ok(shifts)
}

/// Source FK contacts supply airborne height independently of target proportions.
pub fn source_contact_heights(
    psa: &Psa,
    basis: Quat,
    scale: f32,
    first: usize,
    last: usize,
) -> Option<Vec<f32>> {
    use glam::Mat4;
    let foot = |name: &str| {
        psa.bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
    };
    let left = foot("ball_l").or_else(|| foot("foot_l"))?;
    let right = foot("ball_r").or_else(|| foot("foot_r"))?;
    let mut contacts = vec![left, right];
    for name in ["foot_l", "foot_r", "hand_l", "hand_r"] {
        if let Some(i) = foot(name) {
            if !contacts.contains(&i) {
                contacts.push(i);
            }
        }
    }
    let mut floors = Vec::with_capacity(last - first);
    for frame in first..last {
        let mut world = Vec::<Mat4>::with_capacity(psa.bones.len());
        for (i, bone) in psa.bones.iter().enumerate() {
            let k = frame * psa.bones.len() + i;
            let key = &psa.keys[k];
            let local =
                Mat4::from_scale_rotation_translation(psa.scales[k], key.rotation, key.position);
            world.push(bone.parent.map_or(local, |p| world[p] * local));
        }
        floors.push(
            contacts
                .iter()
                .map(|i| (basis * world[*i].w_axis.truncate()).z * scale)
                .fold(f32::INFINITY, f32::min),
        );
    }
    let mut sorted = floors.clone();
    sorted.sort_by(f32::total_cmp);
    // Ignore isolated sub-ground keys when establishing the source contact plane.
    let baseline = sorted[sorted.len() / 100];
    Some(
        floors
            .into_iter()
            .map(|z| (z - baseline).max(0.0))
            .collect(),
    )
}

#[cfg(test)]
mod ground_contact_tests {
    use super::*;
    #[test]
    fn contacts_remove_target_hover_without_flattening_source_jump() {
        let poses: Vec<_> = [0.0, 3.0, 14.0]
            .into_iter()
            .map(|z| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., z]])
            .collect();
        let mesh = serde_json::json!({"bind_floor":0,"floor_z":-24,
            "points":[{"position":[0,0,0],"weights":[[0,1]]}]});
        let shifts = ground_shifts(&poses, 1, &mesh, -24., Some(&[0., 0., 10.])).unwrap();
        let floors: Vec<_> = poses.iter().zip(shifts).map(|(p, s)| p[2][3] + s).collect();
        assert_eq!(floors, [-24., -24., -14.]);
    }
    #[test]
    fn source_contacts_retain_airborne_root_motion() {
        use crate::psa::{Bone, Key};
        let bones = vec![
            Bone {
                name: "root".into(),
                parent: None,
                rest_rotation: Quat::IDENTITY,
                rest_position: Vec3::ZERO,
            },
            Bone {
                name: "foot_l".into(),
                parent: Some(0),
                rest_rotation: Quat::IDENTITY,
                rest_position: Vec3::ZERO,
            },
            Bone {
                name: "foot_r".into(),
                parent: Some(0),
                rest_rotation: Quat::IDENTITY,
                rest_position: Vec3::ZERO,
            },
        ];
        let keys = [0., 7., 0.]
            .into_iter()
            .flat_map(|z| {
                [
                    Key {
                        rotation: Quat::IDENTITY,
                        position: Vec3::new(0., 0., z),
                    },
                    Key {
                        rotation: Quat::IDENTITY,
                        position: Vec3::ZERO,
                    },
                    Key {
                        rotation: Quat::IDENTITY,
                        position: Vec3::ZERO,
                    },
                ]
            })
            .collect();
        let psa = Psa {
            bones,
            keys,
            scales: vec![Vec3::ONE; 9],
            frames: 3,
            fps: 30.,
        };
        assert_eq!(
            source_contact_heights(&psa, Quat::IDENTITY, 1., 0, 3).unwrap(),
            [0., 7., 0.]
        );
    }
}
