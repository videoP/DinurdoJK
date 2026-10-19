//! Opt-in smoke test using local assets, never copied into source control.
use std::path::Path;

#[test]
#[ignore = "requires DJK_MIGRATION_OLD and DJK_MIGRATION_NEW bundle directories"]
fn actual_contact_migration_preserves_all_other_assets() {
    let old = std::env::var("DJK_MIGRATION_OLD").unwrap();
    let new = std::env::var("DJK_MIGRATION_NEW").unwrap();
    let mut bundles = 0;
    let mut audio = 0;
    let mut max_nonheight = 0.0f32;
    for entry in std::fs::read_dir(&new).unwrap().filter_map(Result::ok) {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("djk_fnemote"))
        {
            continue;
        }
        let a = jka_emotes::Bundle::open(&Path::new(&old).join(entry.file_name())).unwrap();
        let b = jka_emotes::Bundle::open(&path).unwrap();
        assert_eq!(
            a.manifest["emotes"],
            b.manifest["emotes"],
            "timeline/audio metadata: {}",
            path.display()
        );
        assert_eq!(
            a.read("animation.gla", jka_emotes::MAX_MEMBER_BYTES)
                .unwrap(),
            b.read("animation.gla", jka_emotes::MAX_MEMBER_BYTES)
                .unwrap()
        );
        let pa = a.pose.as_ref().unwrap();
        let pb = b.pose.as_ref().unwrap();
        assert_eq!((pa.frames, pa.bones), (pb.frames, pb.bones));
        for (a, b) in pa.matrices().iter().zip(pb.matrices()) {
            for r in 0..3 {
                for c in 0..4 {
                    if (r, c) == (2, 3) {
                        continue;
                    }
                    let error = (a[r][c] - b[r][c]).abs();
                    max_nonheight = max_nonheight.max(error);
                    assert!(
                        error <= if c == 3 { 0.00101 } else { 0.0000101 },
                        "non-height coefficient changed: {} ({r},{c}) {error}",
                        path.display()
                    );
                }
            }
        }
        let mut paths = std::collections::HashSet::new();
        for e in jka_emotes::manifest::parse_manifest(&a.manifest).unwrap() {
            for cue in e
                .audio
                .iter()
                .chain(e.variants.iter().flat_map(|v| &v.audio))
            {
                if let Some(path) = &cue.wav {
                    paths.insert(path.clone());
                }
            }
        }
        for path in paths {
            assert_eq!(
                a.read(&path, jka_emotes::MAX_WAV_BYTES).unwrap(),
                b.read(&path, jka_emotes::MAX_WAV_BYTES).unwrap()
            );
            audio += 1;
        }
        bundles += 1;
    }
    println!("CONTACT MIGRATION bundles={bundles} identical_audio={audio} max_nonheight_error={max_nonheight}");
    assert_eq!(
        bundles,
        std::fs::read_dir(old)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e
                .path()
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("djk_fnemote")))
            .count()
    );
}

#[test]
#[ignore = "requires local stock assets and DJK_GROUND_AUDIT_IDS/DJK_GROUND_AUDIT_DIR"]
fn actual_bundle_ground_contact_audit() {
    use jka_assets::{ghoul2::*, skeleton::*};
    let base = std::env::var("DJK_EMOTE_BASE").unwrap();
    let dir = std::env::var("DJK_GROUND_AUDIT_DIR").unwrap();
    let ids: Vec<String> = serde_json::from_slice(
        &std::fs::read(std::env::var("DJK_GROUND_AUDIT_IDS").unwrap()).unwrap(),
    )
    .unwrap();
    let mut assets = jka_assets::pk3::AssetSearchPath::open(Path::new(&base)).unwrap();
    let glm = parse_glm_native(
        &assets
            .read("models/players/kyle/model.glm", 128 * 1024 * 1024)
            .unwrap()
            .unwrap()
            .bytes,
    )
    .unwrap();
    let gameplay = parse_gla(
        &assets
            .read("models/players/_humanoid/_humanoid.gla", 128 * 1024 * 1024)
            .unwrap()
            .unwrap()
            .bytes,
    )
    .unwrap();
    let mesh = if glm.num_bones == 72 {
        humanoid_reference(HumanoidRig::Jk2_72)
    } else {
        std::sync::Arc::new(gameplay)
    };
    let points: Vec<_> = glm.lods[0]
        .surfaces
        .iter()
        .filter(|s| {
            let name = &glm.hierarchy[s.surface_index].name;
            !name.contains("cap") && !name.contains("_off")
        })
        .flat_map(|s| {
            s.vertices.iter().map(|v| {
                (
                    v.position,
                    v.weights
                        .iter()
                        .map(|w| (s.bone_references[w.local_bone_index], w.weight))
                        .collect::<Vec<_>>(),
                )
            })
        })
        .collect();
    let floor = |pose: &[Matrix3x4]| {
        points
            .iter()
            .map(|(p, weights)| {
                weights
                    .iter()
                    .map(|(i, w)| {
                        let m = &pose[*i][2];
                        w * (m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3])
                    })
                    .sum::<f32>()
            })
            .fold(f32::INFINITY, f32::min)
    };
    let mut result = Vec::new();
    for id in ids {
        let path = Path::new(&dir).join(format!("{id}.djk_fnemote"));
        let bundle = jka_emotes::Bundle::open(&path).unwrap();
        let entries = jka_emotes::manifest::parse_manifest(&bundle.manifest).unwrap();
        let track = bundle.pose.as_ref().unwrap();
        let gla = parse_gla(&bundle.read("animation.gla", 128 * 1024 * 1024).unwrap()).unwrap();
        let plan = PoseRetargeter::new(&gla.skeleton, &mesh.skeleton).unwrap();
        for e in &entries[0].variants {
            let mut floors = Vec::new();
            for frame in e.first_frame as usize..e.end_frame as usize {
                let pose = plan
                    .retarget(PoseInput::Ghoul2SkinMatrices(
                        &track.matrices()[frame * track.bones..(frame + 1) * track.bones],
                    ))
                    .unwrap();
                floors.push(floor(&pose));
            }
            let first = floors[0];
            floors.sort_by(f32::total_cmp);
            result.push(serde_json::json!({"id":id,"variant":e.variant_name,"first":first,"min":floors[0],"p05":floors[floors.len()/20],"median":floors[floors.len()/2],"max":floors[floors.len()-1]}));
        }
    }
    let output = std::env::var("DJK_GROUND_AUDIT_OUT").unwrap();
    std::fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
}
#[test]
#[ignore = "requires DJK_EMOTE_BUNDLE and DJK_EMOTE_BASE with licensed local assets"]
fn actual_bundle_skins_selected_model_across_the_full_clip() {
    let bundle_path = std::env::var("DJK_EMOTE_BUNDLE").expect("DJK_EMOTE_BUNDLE");
    let base = std::env::var("DJK_EMOTE_BASE").expect("DJK_EMOTE_BASE");
    let bundle = jka_emotes::Bundle::open(Path::new(&bundle_path)).unwrap();
    let emote = jka_emotes::manifest::parse_manifest(&bundle.manifest)
        .unwrap()
        .remove(0);
    let pose = bundle.pose.as_ref().expect("baked PSA pose track");
    let reference = std::env::var("DJK_EMOTE_REFERENCE_BUNDLE")
        .ok()
        .map(|path| jka_emotes::Bundle::open(Path::new(&path)).unwrap());
    let mut assets = jka_assets::pk3::AssetSearchPath::open(Path::new(&base)).unwrap();
    let model_qpath = bundle.manifest["target_model"]
        .as_str()
        .unwrap_or("models/players/kyle/model.glm");
    let model = jka_assets::ghoul2::parse_glm_native(
        &assets
            .read(model_qpath, 128 * 1024 * 1024)
            .unwrap()
            .unwrap()
            .bytes,
    )
    .unwrap();
    let target = jka_assets::ghoul2::parse_gla(
        &assets
            .read(
                &format!("{}.gla", model.anim_name.trim_end_matches(".gla")),
                128 * 1024 * 1024,
            )
            .unwrap()
            .unwrap()
            .bytes,
    )
    .unwrap();
    let gla =
        jka_assets::ghoul2::parse_gla(&bundle.read("animation.gla", 128 * 1024 * 1024).unwrap())
            .unwrap();
    let target = if model.num_bones == 72 && target.num_bones() == 53 {
        jka_assets::skeleton::humanoid_reference(jka_assets::skeleton::HumanoidRig::Jk2_72)
    } else {
        std::sync::Arc::new(target)
    };
    let plan = jka_assets::skeleton::PoseRetargeter::new(&gla.skeleton, &target.skeleton).unwrap();
    let mut samples = Vec::new();
    let mut first = None;
    let mut last = None;
    let mut clip_floor = f32::INFINITY;
    let mut max_skin_error = 0.0f32;
    for frame in 0..pose.frames {
        let matrices = pose.sample(
            (frame as f64 * 1000.0 / emote.fps as f64).round() as i32,
            emote.fps,
            false,
        );
        let matrices = plan
            .retarget(jka_assets::skeleton::PoseInput::Ghoul2SkinMatrices(
                &matrices,
            ))
            .unwrap();
        let reference_matrices = reference.as_ref().map(|bundle| {
            let original = bundle.pose.as_ref().unwrap();
            assert_eq!((original.frames, original.bones), (pose.frames, pose.bones));
            plan.retarget(jka_assets::skeleton::PoseInput::Ghoul2SkinMatrices(
                &original.sample(
                    (frame as f64 * 1000.0 / emote.fps as f64).round() as i32,
                    emote.fps,
                    false,
                ),
            ))
            .unwrap()
        });
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for surface in &model.lods[0].surfaces {
            // Skip caps and alternate hidden heads; this is a neutral geometry QA.
            let name = &model.hierarchy[surface.surface_index].name;
            if name.contains("_off") || name.contains("cap") {
                continue;
            }
            let skinned = jka_assets::ghoul2::skin_glm_surface(surface, &matrices).unwrap();
            if let Some(reference) = &reference_matrices {
                let exact = jka_assets::ghoul2::skin_glm_surface(surface, reference).unwrap();
                for (a, b) in skinned.vertices.iter().zip(&exact.vertices) {
                    let error = a
                        .position
                        .iter()
                        .zip(b.position)
                        .map(|(a, b)| (a - b).powi(2))
                        .sum::<f32>()
                        .sqrt();
                    max_skin_error = max_skin_error.max(error);
                    assert!(error < 0.01, "compact skin error on frame {frame}: {error}");
                }
            }
            let offset = vertices.len() as u32;
            for v in &skinned.vertices {
                assert!(
                    v.position.iter().all(|n| n.is_finite() && n.abs() < 256.0),
                    "unbounded skin vertex on frame {frame}: {:?}",
                    v.position
                );
                vertices.push(v.position);
            }
            triangles.extend(surface.triangles.iter().map(|t| t.map(|n| n + offset)));
        }
        if frame == 0 {
            let floor = vertices.iter().map(|v| v[2]).fold(f32::INFINITY, f32::min);
            println!("First-frame mesh floor: {floor}");
            if emote.id.starts_with("EID_LaughTrack__") || emote.id.starts_with("EID_DriedSilk__") {
                assert!(
                    (-28.0..-14.0).contains(&floor),
                    "standing emote floats above the native -24 controller floor: {floor}"
                );
            }
            for name in [
                "pelvis", "cranium", "ltibia", "ltalus", "rtibia", "rtalus", "lhand", "rhand",
            ] {
                let index = target
                    .skeleton
                    .iter()
                    .position(|b| b.name.eq_ignore_ascii_case(name))
                    .unwrap();
                let bind = &target.skeleton[index].base_pose;
                let p = [bind[0][3], bind[1][3], bind[2][3]];
                let m = &matrices[index];
                let point: [f32; 3] = std::array::from_fn(|r| {
                    m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3]
                });
                println!("{name}: {point:?}");
            }
            first = Some(vertices.clone());
        }
        clip_floor = vertices.iter().map(|v| v[2]).fold(clip_floor, f32::min);
        if frame == pose.frames - 1 {
            last = Some(vertices.clone());
        }
        if [0, 30, 60, 90, 150, pose.frames - 1].contains(&frame) {
            let joints: std::collections::BTreeMap<_, _> = target
                .skeleton
                .iter()
                .enumerate()
                .filter(|(_, b)| b.name.contains("_d") || b.name == "lhand" || b.name == "rhand")
                .map(|(i, b)| {
                    let p = [b.base_pose[0][3], b.base_pose[1][3], b.base_pose[2][3]];
                    let m = matrices[i];
                    let position: [f32; 3] = std::array::from_fn(|r| {
                        m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3]
                    });
                    (b.name.clone(), position)
                })
                .collect();
            samples
                .push(serde_json::json!({"frame":frame,"vertices":vertices,"triangles":triangles,"joints":joints}));
        }
    }
    assert_ne!(first, last, "emote must change the posed mesh");
    if model_qpath == "models/players/kyle/model.glm"
        && (emote.id.starts_with("EID_LaughTrack") || emote.id.starts_with("EID_DriedSilk"))
    {
        assert!(
            (-26.0..-22.0).contains(&clip_floor),
            "emote fails to reach native standing ground height: {clip_floor}"
        );
    }
    println!("Full-clip lowest mesh point: {clip_floor}");
    println!("Maximum compact-versus-exact posed vertex error: {max_skin_error}");
    let path = Path::new("../../target/emote-qa");
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(
        path.join("frames.json"),
        serde_json::to_vec(&samples).unwrap(),
    )
    .unwrap();
    println!(
        "Verified {} posed frames on {} surfaces; wrote target/emote-qa/frames.json",
        pose.frames,
        model.lods[0].surfaces.len()
    );
}
