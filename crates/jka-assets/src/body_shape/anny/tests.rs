use super::*;
use std::io::Read;

#[test]
fn port_matches_independent_python_evaluator() {
    let data = model().unwrap();
    let vertices = data.evaluate(BodyShapeParams::default(), Details::default());
    let expected = include_bytes!("../../../tests/data/anny-neutral-oracle.bin");
    assert_eq!(expected.len(), vertices.len() * 12);
    let mut worst = 0.0f32;
    for (actual, raw) in vertices.iter().flatten().zip(expected.chunks_exact(4)) {
        worst = worst.max((actual - f32::from_le_bytes(raw.try_into().unwrap())).abs());
    }
    assert!(worst < 5e-6, "Anny source parity: {worst}");
}

#[test]
fn named_controls_roundtrip_and_reject_invalid_input() {
    let text = "gender=0.25,cupsize=0.8,buttocks-volume-incr=-0.65,jawOpen=0.4";
    let details = Details::parse(text).unwrap();
    assert_eq!(Details::parse(&details.encode()).unwrap(), details);
    assert!(Details::parse("").unwrap().is_neutral());
    assert!(Details::parse("unknown=1").is_err());
    assert!(Details::parse("gender=NaN").is_err());
    assert!(Details::parse("gender").is_err());
    assert_eq!(Details::parse("gender=5").unwrap().0[0], 1.);
    assert_ne!(
        fingerprint(BodyShapeParams::default(), details),
        fingerprint(BodyShapeParams::default(), Details::default())
    );
}

#[test]
fn retopology_projects_triangles_edges_and_degenerate_faces() {
    let triangle = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]];
    let (weights, distance) = closest_triangle([0.25, 0.5, 2.], triangle);
    assert!((distance - 4.).abs() < 1e-6);
    assert_eq!(weights, [0.25, 0.25, 0.5]);
    let (weights, distance) = closest_triangle([2., 0., 0.], triangle);
    assert_eq!(weights, [0., 1., 0.]);
    assert_eq!(distance, 1.);
    let (weights, distance) = closest_triangle([1., 0., 0.], [[0.; 3]; 3]);
    assert!(weights.iter().all(|x| x.is_finite()));
    assert_eq!(distance, 1.);
}

#[test]
fn retopology_preserves_authored_shapes_on_the_donor_surface() {
    use crate::{
        ghoul2::*,
        skeleton::{humanoid_reference, HumanoidRig},
    };
    let data = model().unwrap();
    let gla = humanoid_reference(HumanoidRig::Jka53);
    let mut native: Vec<_> = gla
        .skeleton
        .iter()
        .map(|b| std::array::from_fn(|k| b.base_pose[k][3]))
        .collect();
    for (i, b) in data.meta.bones.iter().enumerate() {
        if let Some(target) = gla.skeleton.iter().position(|t| t.name == b.target) {
            native[target] = data.heads[i];
        }
    }
    let mut glm = GlmModel {
        name: "anny parity".into(),
        anim_name: String::new(),
        num_bones: gla.skeleton.len(),
        hierarchy: vec![],
        lods: vec![GlmLod {
            surfaces: vec![GlmSurface {
                surface_index: 0,
                vertices: data
                    .neutral
                    .iter()
                    .map(|&position| GlmVertex {
                        position,
                        normal: [0., -1., 0.],
                        weights: vec![],
                    })
                    .collect(),
                texcoords: vec![[0.; 2]; data.neutral.len()],
                triangles: data.faces.iter().map(|f| f.map(|i| i as u32)).collect(),
                bone_references: vec![],
            }],
        }],
    };
    let params = BodyShapeParams::default();
    let details =
        Details::parse("gender=0.8,cupsize=0.7,buttocks-volume-incr=-0.6,jawOpen=0.5").unwrap();
    let expected = data.evaluate(params, details);
    // Click classification on the source surface must resolve distinct authored
    // regions, rather than returning the same fallback group for every hit.
    let mut selected = std::collections::HashSet::new();
    let mut selection_points = Vec::new();
    for control in [
        "l-upperarm-fat-incr",
        "buttocks-volume-incr",
        "head-scale-vert-incr",
    ] {
        if let Some(i) = LOCAL_CONTROLS.iter().position(|c| c.0 == control) {
            let mut offsets = vec![[0.; 3]; data.neutral.len()];
            data.accumulate(&mut offsets, &data.meta.controls[i].sides[0], 1.);
            let vertex = offsets
                .iter()
                .enumerate()
                .max_by(|a, b| length(*a.1).total_cmp(&length(*b.1)))
                .unwrap()
                .0;
            selected.insert(region(&glm, &gla, Some(&native), data.neutral[vertex]).unwrap());
            selection_points.push(data.neutral[vertex]);
        }
    }
    assert!(
        selected.len() >= 2,
        "body selection did not distinguish authored regions: {selected:?}"
    );
    let expected_regions: Vec<_> = selection_points
        .iter()
        .map(|&p| region(&glm, &gla, Some(&native), p).unwrap())
        .collect();
    assert_eq!(
        regions(&glm, &gla, Some(&native), selection_points).unwrap(),
        expected_regions,
        "highlight masks must use the same regions as body picking"
    );
    apply(&mut glm, &gla, params, details, Some(&native)).unwrap();
    let vertices = &glm.lods[0].surfaces[0].vertices;
    let mut worst = 0.0f32;
    for face in &data.faces {
        for &id in face {
            worst = worst.max(length(sub(vertices[id].position, expected[id])));
        }
    }
    assert!(worst < 1e-4, "donor retopology drift {worst}");
}

#[test]
fn macro_changes_preserve_authored_segment_ratios_on_game_rigs() {
    use crate::{
        ghoul2::*,
        skeleton::{humanoid_reference, HumanoidRig},
    };
    let data = model().unwrap();
    for rig in [HumanoidRig::Jka53, HumanoidRig::Jk2_72] {
        let gla = humanoid_reference(rig);
        let ids: Vec<_> = data
            .meta
            .bones
            .iter()
            .map(|b| gla.skeleton.iter().position(|t| t.name == b.target))
            .collect();
        let stock: Vec<[f32; 3]> = gla
            .skeleton
            .iter()
            .map(|b| std::array::from_fn(|k| b.base_pose[k][3]))
            .collect();
        // Include a differently proportioned imported bind skeleton, not only
        // the source donor's identity fit that previously hid this regression.
        let native: Vec<_> = stock
            .iter()
            .map(|p| [p[0] * 0.8, p[1] * 1.2, p[2] * 0.9])
            .collect();
        for original in [None, Some(native.as_slice())] {
            let rest = original.unwrap_or(&stock);
            for axis in [
                "gender",
                "cupsize",
                "firmness",
                "buttocks-volume-incr",
                "jawOpen",
            ] {
                for value in [0.0, 0.25, 0.75, 1.0] {
                    let params = BodyShapeParams::default();
                    let mut details = Details::parse(&format!("{axis}={value}")).unwrap();
                    // Nonneutral details exercise transfer even at a control's neutral anchor.
                    details.0[0] = if axis == "gender" { value } else { 0.7 };
                    let expected = data.evaluate(params, details);
                    let heads: Vec<_> = data
                        .meta
                        .bones
                        .iter()
                        .map(|b| mean(&expected, &b.head))
                        .collect();
                    let mut glm = GlmModel {
                        name: "joint regression".into(),
                        anim_name: String::new(),
                        num_bones: gla.skeleton.len(),
                        hierarchy: vec![],
                        lods: vec![GlmLod { surfaces: vec![] }],
                    };
                    let pivots = apply(&mut glm, &gla, params, details, original)
                        .unwrap()
                        .unwrap();
                    for (i, &id) in ids.iter().enumerate() {
                        let Some(id) = id else {
                            continue;
                        };
                        let mut parent = data
                            .meta
                            .bones
                            .iter()
                            .position(|b| b.name == data.meta.bones[i].parent);
                        while let Some(p) = parent {
                            if let Some(pid) = ids[p] {
                                let before = length(sub(data.heads[i], data.heads[p]));
                                if before > 1e-6 {
                                    let ratio = length(sub(heads[i], heads[p])) / before;
                                    let expected_length = length(sub(rest[id], rest[pid])) * ratio;
                                    let actual = length(sub(pivots[id], pivots[pid]));
                                    assert!((actual - expected_length).abs() < 2e-4,
                                        "{rig:?} native={} {axis}={value} {}: {actual} != {expected_length}",
                                        original.is_some(), data.meta.bones[i].name);
                                }
                                break;
                            }
                            parent = data
                                .meta
                                .bones
                                .iter()
                                .position(|b| b.name == data.meta.bones[p].parent);
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "requires installed retail PK3s"]
fn retail_models_keep_neutral_geometry_and_animate_authored_local_targets() {
    use crate::ghoul2::*;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // This fixture intentionally reads only retail archives; unrelated addon
    // packages can have independent ACLs or hold exclusive writer locks.
    let mut archives: Vec<_> = (0..4)
        .map(|i| {
            zip::ZipArchive::new(
                std::fs::File::open(root.join(format!("target/release/base/assets{i}.pk3")))
                    .unwrap(),
            )
            .unwrap()
        })
        .collect();
    let mut read = |name: &str| {
        for archive in archives.iter_mut().rev() {
            if let Ok(mut entry) = archive.by_name(name) {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                return bytes;
            }
        }
        panic!("missing retail fixture {name}");
    };
    for name in ["jedi_hf", "jedi_hm", "jedi_zf"] {
        let raw = read(&format!("models/players/{name}/model.glm"));
        let original = parse_glm_native(&raw).unwrap();
        let animation = read(&format!("{}.gla", original.anim_name));
        let gla = parse_gla(&animation).unwrap();
        assert!(supports(&original, &gla, None), "{name}");
        let mut neutral = original.clone();
        assert!(apply(
            &mut neutral,
            &gla,
            BodyShapeParams::default(),
            Details::default(),
            None
        )
        .unwrap()
        .is_none());
        assert_eq!(
            super::super::binding_fingerprint(&neutral),
            super::super::binding_fingerprint(&original)
        );
        let neutral = BodyShapeParams::default();
        for (params, recipe) in [
            (neutral, "buttocks-volume-incr=0.6"),
            (neutral, "l-upperarm-fat-incr=-0.6"),
            (neutral, "jawOpen=0.5"),
            (neutral, "gender=0.8,cupsize=0.2"),
            (
                BodyShapeParams {
                    height: 0.,
                    ..neutral
                },
                "",
            ),
            (
                BodyShapeParams {
                    height: 1.,
                    ..neutral
                },
                "",
            ),
            (BodyShapeParams { age: 0., ..neutral }, ""),
            (BodyShapeParams { age: 1., ..neutral }, ""),
            (
                BodyShapeParams {
                    height: 0.,
                    age: 0.,
                    ..neutral
                },
                "gender=0.8,cupsize=0.2,buttocks-volume-incr=-0.6,jawOpen=0.5",
            ),
            (
                BodyShapeParams {
                    height: 1.,
                    age: 1.,
                    ..neutral
                },
                "gender=0.2,firmness=0.8,l-upperarm-fat-incr=0.6",
            ),
        ] {
            let mut shaped = original.clone();
            let pivots = apply_profile(
                &mut shaped,
                &gla,
                params,
                Details::parse(recipe).unwrap(),
                None,
            )
            .unwrap()
            .unwrap();
            if recipe.is_empty() {
                let mut previous = original.clone();
                let previous_pivots =
                    super::super::procedural::apply(&mut previous, &gla, params, None)
                        .unwrap()
                        .unwrap();
                assert_eq!(
                    pivots, previous_pivots,
                    "restore previous pivots: {name} {params:?}"
                );
                assert_eq!(
                    super::super::binding_fingerprint(&shaped),
                    super::super::binding_fingerprint(&previous),
                    "restore previous mesh: {name} {params:?}"
                );
            }
            let binding = super::super::BodyShapeBinding::new(&gla, None, &pivots).unwrap();
            let poses: Vec<_> = [14651., 20447., 2947.]
                .into_iter()
                .map(|frame| {
                    let mut animator = Ghoul2Animator::new(&gla);
                    animator
                        .set_bone_anim(
                            &gla,
                            "model_root",
                            0,
                            (gla.num_frames - 1) as i32,
                            BONE_ANIM_OVERRIDE_FREEZE,
                            1.,
                            0,
                            Some(frame),
                            0,
                        )
                        .unwrap();
                    binding
                        .retarget(&gla, &animator.evaluate_pose_openjk_root(&gla, 0).unwrap())
                        .unwrap()
                })
                .collect();
            let mut changed = 0.;
            for (old, new) in original.lods.iter().zip(&shaped.lods) {
                for (old, new) in old.surfaces.iter().zip(&new.surfaces) {
                    assert_eq!(old.triangles, new.triangles);
                    assert_eq!(old.texcoords, new.texcoords);
                    for (a, b) in old.vertices.iter().zip(&new.vertices) {
                        changed += length(sub(a.position, b.position));
                        assert_eq!(a.weights, b.weights);
                    }
                    for pose in &poses {
                        let skin = skin_glm_surface(new, pose).unwrap();
                        assert!(skin.vertices.iter().all(|v| v
                            .position
                            .iter()
                            .chain(&v.normal)
                            .all(|x| x.is_finite())));
                    }
                }
            }
            assert!(changed > 0., "{name}: {recipe} has no influence");
        }
    }
}

#[test]
fn runtime_data_is_trimmed_and_keeps_all_detail_controls() {
    let data = model().unwrap();
    assert_eq!(data.meta.targets.len(), 48);
    assert_eq!(data.meta.controls.len(), 256);
    assert_eq!(data.meta.facial.len(), 52);
    assert!(data.bytes.len() < 20 * 1024 * 1024);
    for i in 0..DETAIL_COUNT {
        for value in [-1.0, 1.0] {
            let mut details = Details::default();
            details.0[i] = value;
            let vertices = data.evaluate(BodyShapeParams::default(), details);
            assert!(
                vertices.iter().flatten().all(|x| x.is_finite()),
                "{}={value}",
                Details::name(i)
            );
        }
    }
}

#[test]
fn malformed_runtime_data_returns_an_error() {
    assert!(Model::parse(b"not JSON", Vec::new()).is_err());
    assert!(Model::parse(b"{}", Vec::new()).is_err());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut assets = crate::pk3::AssetSearchPath::open(&root).unwrap();
    let json = assets
        .read(RUNTIME_METADATA, 2 * 1024 * 1024)
        .unwrap()
        .unwrap()
        .bytes;
    assert!(
        Model::parse(&json, Vec::new()).is_err(),
        "missing array payload must be rejected"
    );
}

#[test]
fn detail_api_rejects_unavailable_whole_body_axes() {
    use crate::{
        ghoul2::*,
        skeleton::{humanoid_reference, HumanoidRig},
    };
    let gla = humanoid_reference(HumanoidRig::Jka53);
    let mut glm = GlmModel {
        name: "runtime contract".into(),
        anim_name: String::new(),
        num_bones: gla.skeleton.len(),
        hierarchy: vec![],
        lods: vec![],
    };
    let error = apply(
        &mut glm,
        &gla,
        BodyShapeParams {
            height: 1.,
            ..Default::default()
        },
        Details::default(),
        None,
    )
    .unwrap_err();
    assert!(error.contains("apply_profile"));
}
