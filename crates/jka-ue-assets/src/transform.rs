//! FTransform and skeletal reference poses in the game's reflected coordinates.
//! Reference: UE SceneComponent::CalcNewComponentToWorld_GeneralCase and
//! SkeletalRender::UpdateRefToLocalMatrices (mesh RefBasesInvMatrix).
use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

fn vector(value: &Value, default: Vec3) -> Result<Vec3, String> {
    if value.is_null() {
        return Ok(default);
    }
    let mut result = [0.; 3];
    for (i, key) in ["X", "Y", "Z"].iter().enumerate() {
        result[i] = value[key].as_f64().ok_or("invalid UE vector")? as f32;
    }
    let result = Vec3::from_array(result);
    if !result.is_finite() {
        return Err("non-finite UE vector".into());
    }
    Ok(result)
}
pub fn local(value: &Value) -> Result<Mat4, String> {
    let r = &value["Rotation"];
    let q = if r.is_null() {
        Quat::IDENTITY
    } else {
        let mut components = [0.; 4];
        for (i, key) in ["X", "Y", "Z", "W"].iter().enumerate() {
            components[i] = r[key].as_f64().ok_or("invalid UE quaternion")? as f32;
        }
        let q = Quat::from_array(components);
        if !q.is_finite() || q.length_squared() < 1e-12 {
            return Err("invalid UE quaternion".into());
        }
        q.normalize()
    };
    let mut t = vector(&value["Translation"], Vec3::ZERO)?;
    t.y = -t.y;
    let s = vector(&value["Scale3D"], Vec3::ONE)?;
    if s.abs().min_element() < 1e-8 {
        return Err("singular mesh reference transform".into());
    }
    Ok(Mat4::from_scale_rotation_translation(
        s,
        Quat::from_xyzw(-q.x, q.y, -q.z, q.w),
        t,
    ))
}

/// Preserve the *mesh* reference pose, including scale. Animation USkeleton
/// bind poses may differ and must never be substituted for these inverses.
pub fn reference_matrices(bones: &Value) -> Result<Vec<Mat4>, String> {
    let bones = bones
        .as_array()
        .filter(|b| !b.is_empty() && b.len() <= 4096)
        .ok_or("invalid mesh reference skeleton")?;
    let mut matrices = Vec::with_capacity(bones.len());
    for (index, bone) in bones.iter().enumerate() {
        let local = local(&bone["Transform"])?;
        let parent = bone["ParentIndex"]
            .as_i64()
            .ok_or("missing mesh reference parent")?;
        let world = if parent == -1 {
            local
        } else {
            let parent = usize::try_from(parent).map_err(|_| "invalid reference parent")?;
            if parent >= index {
                return Err("reference skeleton parent must precede child".into());
            }
            matrices[parent] * local
        };
        if !world.is_finite() || world.determinant().abs() < 1e-12 {
            return Err("singular mesh reference skeleton".into());
        }
        matrices.push(world);
    }
    Ok(matrices)
}

#[derive(Default, Clone, Copy)]
pub struct Absolute {
    pub translation: bool,
    pub rotation: bool,
    pub scale: bool,
}
/// Unreal child relative transform composed with its parent's socket, including
/// the three independent Absolute flags used by Blueprint SceneComponents.
pub fn component_world(parent: Mat4, relative: Mat4, absolute: Absolute) -> Mat4 {
    let combined = parent * relative;
    let (mut scale, mut rotation, mut translation) = combined.to_scale_rotation_translation();
    let (s, r, t) = relative.to_scale_rotation_translation();
    if absolute.translation {
        translation = t
    }
    if absolute.rotation {
        rotation = r
    }
    if absolute.scale {
        scale = s
    }
    Mat4::from_scale_rotation_translation(scale, rotation, translation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reference_pose_includes_mesh_scale_and_reflects_y() {
        let bones = json!([{"ParentIndex":-1,"Transform":{"Translation":{"X":0,"Y":5,"Z":0},"Scale3D":{"X":2,"Y":2,"Z":2}}},{"ParentIndex":0,"Transform":{"Translation":{"X":1,"Y":3,"Z":0}}}]);
        let rest = reference_matrices(&bones).unwrap();
        assert_eq!(
            rest[1].transform_point3(Vec3::ZERO),
            Vec3::new(2., -11., 0.)
        );
        assert!((rest[1] * rest[1].inverse() - Mat4::IDENTITY).abs_diff_eq(Mat4::ZERO, 1e-6));
    }
    #[test]
    fn absolute_location_keeps_socket_rotation() {
        let parent = Mat4::from_rotation_translation(Quat::from_rotation_z(1.), Vec3::splat(30.));
        let relative = Mat4::from_translation(Vec3::X);
        let actual = component_world(
            parent,
            relative,
            Absolute {
                translation: true,
                ..Default::default()
            },
        );
        assert_eq!(actual.w_axis.truncate(), Vec3::X);
        assert!(actual.x_axis.abs_diff_eq(parent.x_axis, 1e-6));
    }
    #[test]
    fn reference_parent_cycles_and_nonfinite_transforms_fail() {
        assert!(reference_matrices(&json!([{"ParentIndex":0,"Transform":{}}])).is_err());
        assert!(local(&json!({"Scale3D":{"X":0,"Y":1,"Z":1}})).is_err());
    }
}
