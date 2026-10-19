use glam::{Quat, Vec3};
use jka_assets::ghoul2::{openjk_default_gla, GlaBone};
use jka_emotes::{
    psa::{Bone, Key, Psa},
    retarget, PoseClip,
};
use serde_json::json;

fn fixtures() -> (Psa, jka_assets::ghoul2::GlaAnimation) {
    let names = [
        ("pelvis", "pelvis", None, Vec3::new(0.0, 0.0, 10.0)),
        ("thoracic", "spine_05", Some(0), Vec3::new(0.0, 0.0, 5.0)),
        ("cranium", "head", Some(1), Vec3::new(0.0, 0.0, 5.0)),
        ("lfemurYZ", "thigh_l", Some(0), Vec3::new(0.0, 2.0, 0.0)),
        ("rfemurYZ", "thigh_r", Some(0), Vec3::new(0.0, -2.0, 0.0)),
        ("lhumerus", "upperarm_l", Some(1), Vec3::new(0.0, 4.0, 0.0)),
        ("rhumerus", "upperarm_r", Some(1), Vec3::new(0.0, -4.0, 0.0)),
    ];
    let mut target = openjk_default_gla().unwrap();
    target.skeleton.clear();
    let mut positions = Vec::new();
    let mut bones = Vec::new();
    for (i, (dst, src, parent, local)) in names.into_iter().enumerate() {
        let p = parent.map_or(local, |p| positions[p] + local);
        positions.push(p);
        let base = [
            [0.64, 0.0, 0.0, p.x],
            [0.0, 0.64, 0.0, p.y],
            [0.0, 0.0, 0.64, p.z],
        ];
        let inv = [
            [1.0 / 0.64, 0.0, 0.0, -p.x / 0.64],
            [0.0, 1.0 / 0.64, 0.0, -p.y / 0.64],
            [0.0, 0.0, 1.0 / 0.64, -p.z / 0.64],
        ];
        target.skeleton.push(GlaBone {
            name: dst.into(),
            flags: 0,
            parent: parent.map_or(-1, |n| n as i32),
            base_pose: base,
            base_pose_inv: inv,
            children: vec![],
        });
        if let Some(p) = parent {
            target.skeleton[p].children.push(i);
        }
        bones.push(Bone {
            name: src.into(),
            parent,
            rest_rotation: Quat::IDENTITY,
            rest_position: local,
        });
    }
    let bytes =
        retarget::encode(&target, 1, &vec![(Quat::IDENTITY, Vec3::ZERO); bones.len()]).unwrap();
    let target = jka_assets::ghoul2::parse_gla(&bytes).unwrap();
    let keys = bones
        .iter()
        .map(|b| Key {
            rotation: b.rest_rotation,
            position: b.rest_position,
        })
        .collect();
    let psa = Psa {
        scales: vec![Vec3::ONE; bones.len()],
        bones,
        frames: 1,
        fps: 30.0,
        keys,
    };
    (psa, target)
}
#[test]
fn fingers_use_middle_knuckle_motion_relative_to_the_hand() {
    let (mut psa, mut target) = fixtures();
    let extras = [
        ("lhand", "hand_l", 5, Vec3::new(2.0, 0.0, 0.0)),
        ("l_d2_j1", "index_01_l", 7, Vec3::new(2.0, 1.0, 0.0)),
        ("l_d2_j2", "index_02_l", 8, Vec3::new(2.0, 0.0, 0.0)),
        ("l_d4_j1", "ring_01_l", 7, Vec3::new(2.0, -1.0, 0.0)),
        ("l_d4_j2", "ring_02_l", 10, Vec3::new(2.0, 0.0, 0.0)),
    ];
    for (dst, src, parent, local) in extras {
        let base = &target.skeleton[parent].base_pose;
        let position = Vec3::new(base[0][3], base[1][3], base[2][3]) + local;
        let index = target.skeleton.len();
        target.skeleton[parent].children.push(index);
        target.skeleton.push(GlaBone {
            name: dst.into(),
            flags: 0,
            parent: parent as i32,
            children: vec![],
            base_pose: [
                [1.0, 0.0, 0.0, position.x],
                [0.0, 1.0, 0.0, position.y],
                [0.0, 0.0, 1.0, position.z],
            ],
            base_pose_inv: [
                [1.0, 0.0, 0.0, -position.x],
                [0.0, 1.0, 0.0, -position.y],
                [0.0, 0.0, 1.0, -position.z],
            ],
        });
        psa.bones.push(Bone {
            name: src.into(),
            parent: Some(parent),
            rest_position: local,
            rest_rotation: Quat::IDENTITY,
        });
        psa.keys.push(Key {
            position: local,
            rotation: Quat::IDENTITY,
        });
        psa.scales.push(Vec3::ONE);
    }
    // The UE distal joint has a very different motion. It must not be folded
    // into JKA's middle joint, which previously mapped straight to _03.
    for (name, parent) in [("index_03_l", 9), ("pinky_03_l", 11)] {
        psa.bones.push(Bone {
            name: name.into(),
            parent: Some(parent),
            rest_position: Vec3::X,
            rest_rotation: Quat::IDENTITY,
        });
        psa.keys.push(Key {
            position: Vec3::X,
            rotation: Quat::from_rotation_z(2.0),
        });
        psa.scales.push(Vec3::ONE);
    }
    let frame = psa.keys.clone();
    psa.keys.extend(frame);
    psa.frames = 2;
    psa.scales.resize(psa.keys.len(), Vec3::ONE);
    let bones = psa.bones.len();
    psa.keys[bones + 7].rotation = Quat::from_rotation_x(0.6);
    psa.keys[bones + 9].rotation = Quat::from_rotation_z(0.3);
    let target = jka_assets::ghoul2::parse_gla(
        &retarget::encode(
            &target,
            1,
            &vec![(Quat::IDENTITY, Vec3::ZERO); target.num_bones()],
        )
        .unwrap(),
    )
    .unwrap();
    let (_, bytes, report) = retarget::bake(
        &psa,
        &target,
        &json!({"ground_contact":false,"origin_offset_z":0}),
    )
    .unwrap();
    assert_eq!(report["bone_map"]["l_d2_j2"], "index_02_l");
    let pose = PoseClip::parse(&bytes).unwrap();
    let first = pose.sample(0, 30.0, false);
    let second = pose.sample(34, 30.0, false);
    let matrix = |m: &[[f32; 4]; 3]| {
        glam::Mat3::from_cols_array(&[
            m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2],
        ])
    };
    assert!(matrix(&first[9]).abs_diff_eq(matrix(&first[8]), 0.0001));
    let local = matrix(&second[8]).inverse() * matrix(&second[9]);
    assert!(
        local.abs_diff_eq(glam::Mat3::from_quat(Quat::from_rotation_z(0.3)), 0.0001),
        "{local:?}"
    );
}
#[test]
fn ground_calibration_is_constant_and_does_not_flatten_jumps() {
    let (mut psa, target) = fixtures();
    let mut first = psa.keys.clone();
    first[0].position.z += 2.0;
    let mut second = psa.keys.clone();
    second[0].position.z += 5.0;
    psa.keys = first.into_iter().chain(second).collect();
    psa.frames = 2;
    psa.scales.resize(psa.keys.len(), Vec3::ONE);
    let settings = json!({"origin_offset_z":-24,"ground_mesh":{"bind_floor":0,"points":[{"position":[0,0,0],"weights":[[0,1]]}]}});
    let (gla, bytes, _) = retarget::bake(&psa, &target, &settings).unwrap();
    let pose = PoseClip::parse(&bytes).unwrap();
    assert!((pose.sample(0, 30.0, false)[0][2][3] + 24.0).abs() < 0.001);
    assert!((pose.sample(34, 30.0, false)[0][2][3] + 21.0).abs() < 0.001);
    let gla = jka_assets::ghoul2::parse_gla(&gla).unwrap();
    assert!((gla.decompress_bone(0, 0).unwrap()[2][3] + 24.0).abs() < 0.02);
}
#[test]
fn native_origin_translation_is_applied_once_and_hip_motion_survives() {
    let (mut psa, target) = fixtures();
    let mut locals = vec![(Quat::IDENTITY, Vec3::ZERO); target.num_bones()];
    locals[0].1.z = -24.0;
    let target =
        jka_assets::ghoul2::parse_gla(&retarget::encode(&target, 1, &locals).unwrap()).unwrap();
    psa.keys[0].position.z += 3.0;
    let (_, bytes, _) = retarget::bake(&psa, &target, &json!({})).unwrap();
    for m in PoseClip::parse(&bytes).unwrap().sample(0, 30.0, false) {
        assert!(
            (m[2][3] + 21.0).abs() < 0.001,
            "root offset or jump applied twice: {m:?}"
        );
    }
}
#[test]
fn rest_pose_produces_identity_skin_transforms_with_scaled_target_bind() {
    let (psa, target) = fixtures();
    let (_, bytes, _) = retarget::bake(&psa, &target, &json!({})).unwrap();
    let pose = PoseClip::parse(&bytes).unwrap().sample(0, 30.0, false);
    for m in pose {
        for (r, row) in m.iter().enumerate() {
            for (c, n) in row.iter().enumerate() {
                assert!(
                    (n - jka_assets::ghoul2::OPENJK_GHOUL2_ROOT_MATRIX[r][c]).abs() < 0.0001,
                    "rest pose deforms skin: {m:?}"
                );
            }
        }
    }
}
#[test]
fn animated_scale_survives_baking_and_target_skeleton_is_preserved() {
    let (mut psa, target) = fixtures();
    psa.scales[1] = Vec3::splat(1.2);
    let (gla, bytes, _) = retarget::bake(&psa, &target, &json!({})).unwrap();
    jka_emotes::manifest::validate_gla(&target, &jka_assets::ghoul2::parse_gla(&gla).unwrap())
        .unwrap();
    let pose = PoseClip::parse(&bytes).unwrap().sample(0, 30.0, false);
    assert!((pose[1][1][0] - 1.2).abs() < 0.0001);
    assert!((pose[2][1][0] - 1.2).abs() < 0.0001);
}
#[test]
fn explicit_missing_bones_and_invalid_trim_fail() {
    let (psa, target) = fixtures();
    assert!(retarget::bake(&psa, &target, &json!({"bone_map":{"pelvis":"absent"}})).is_err());
    assert!(retarget::bake(&psa, &target, &json!({"end_seconds":99})).is_err());
}
#[test]
fn zero_scale_preserves_affine_pose_with_a_finite_rigid_fallback() {
    let (mut psa, target) = fixtures();
    psa.scales[0] = Vec3::ZERO;
    let (gla, bytes, report) = retarget::bake(&psa, &target, &json!({})).unwrap();
    assert!(report["rigid_fallback_transforms"].as_u64().unwrap() > 0);
    let pose = PoseClip::parse(&bytes).unwrap().sample(0, 30.0, false);
    assert!(pose.iter().flatten().flatten().all(|v| v.is_finite()));
    assert_eq!(pose[0][0][0], 0.0);
    jka_assets::ghoul2::parse_gla(&gla)
        .unwrap()
        .compose_frame(0)
        .unwrap();
}
#[test]
fn short_rigs_and_custom_names_use_the_mapped_anatomical_landmarks() {
    let (mut psa, target) = fixtures();
    for (bone, name) in psa.bones.iter_mut().zip([
        "pelvis", "torso", "head", "leg_l", "leg_r", "arm_l", "arm_r",
    ]) {
        bone.name = name.into();
    }
    let (_, bytes, _) = retarget::bake(&psa, &target, &json!({})).unwrap();
    assert!(PoseClip::parse(&bytes)
        .unwrap()
        .sample(0, 30.0, false)
        .iter()
        .flatten()
        .flatten()
        .all(|v| v.is_finite()));
    psa.bones[2].name = "custom_head".into();
    retarget::bake(
        &psa,
        &target,
        &json!({"bone_map":{"cranium":"custom_head"}}),
    )
    .unwrap();
}
