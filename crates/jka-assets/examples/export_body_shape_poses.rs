//! Offline regression capture using the same shape binding and Ghoul2 skinner
//! as the client. Does not start or control a game process.
//! Usage: export_body_shape_poses <base> <anny-output-directory>
use jka_assets::{
    body_shape::{model_fingerprint, BodyShapeBinding, BodyShapeGrid, BodyShapeParams},
    ghoul2::*,
    pk3::AssetSearchPath,
    skeleton::{anatomical_parent, humanoid_reference, HumanoidRig, PoseInput, PoseRetargeter},
    source1::{parse_source1_materials, parse_source1_skeleton},
    source1_geometry::{parse_source_vtx, parse_source_vvd},
    source1_rig::{
        attach_humanoid_bolts_between_rigs, build_source_rig, rebase_source_ghoul2_pose,
        SourceRigOptions,
    },
};
use std::{
    io::{BufWriter, Write},
    path::Path,
};
fn point(m: &Matrix3x4, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| m[k][3] + (0..3).map(|i| m[k][i] * p[i]).sum::<f32>())
}
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}
fn write_u32(w: &mut impl Write, x: usize) -> std::io::Result<()> {
    w.write_all(&(x as u32).to_le_bytes())
}
fn write_name(w: &mut impl Write, s: &str) -> std::io::Result<()> {
    write_u32(w, s.len())?;
    w.write_all(s.as_bytes())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("expected base and output directory".into());
    }
    let out = Path::new(&args[1]);
    let mut assets = AssetSearchPath::open_game(Path::new(&args[0]), None)?;
    assets.set_allow_asset_overrides(true);
    let mut read = |q: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(assets
            .read(q, 128 * 1024 * 1024)?
            .ok_or_else(|| format!("missing {q}"))?
            .bytes)
    };
    let animation = parse_gla(&read("models/players/_humanoid/_humanoid.gla")?)?;
    // Actual animation.cfg clips: BOTH_STAND1, BOTH_WALK1, BOTH_CROUCH1.
    let frames: Vec<usize> = (14651..14653)
        .chain(20438..20464)
        .chain(2944..2948)
        .collect();
    let mut stocks = Vec::new();
    for &frame in &frames {
        let mut animator = Ghoul2Animator::new(&animation);
        animator.set_bone_anim(
            &animation,
            "model_root",
            0,
            (animation.num_frames - 1) as i32,
            BONE_ANIM_OVERRIDE_FREEZE,
            1.,
            0,
            Some(frame as f32),
            0,
        )?;
        stocks.push(animator.evaluate_pose_openjk_root(&animation, 0)?);
    }
    let cases: [(&str, [f32; 4]); 12] = [
        ("neutral", [0.5; 4]),
        ("weight_low", [0.5, 0., 0.5, 0.5]),
        ("weight_high", [0.5, 1., 0.5, 0.5]),
        ("muscle_low", [0., 0.5, 0.5, 0.5]),
        ("muscle_high", [1., 0.5, 0.5, 0.5]),
        ("bulky", [1., 1., 0.5, 0.5]),
        ("proportions_low", [0.5, 0.5, 0., 0.5]),
        ("proportions_high", [0.5, 0.5, 1., 0.5]),
        ("child", [0.5, 0.5, 0.5, 0.]),
        ("elder", [0.5, 0.5, 0.5, 1.]),
        ("child_heavy", [0., 1., 0.5, 0.]),
        ("child_bulky", [1., 1., 0.5, 0.]),
    ];
    let mut diagnostics =
        BufWriter::new(std::fs::File::create(out.join("animated_link_checks.csv"))?);
    writeln!(
        diagnostics,
        "model,case,frames,bone_links,max_link_error,visible_vertices,bundle_fingerprint"
    )?;
    for model in ["jedi_hm", "jedi_hf", "jedi_zf", "temp"] {
        let (raw, original, gla, original_pivots, mut source_rig) = if model == "temp" {
            let stem="models/player/fwhykay/lollipop_chainsaw/juliet_starling_cine/juliet_starling_american_casual/juliet_starling_american_casual";
            let raw = read(&format!("{stem}.mdl"))?;
            let vvd = parse_source_vvd(&read(&format!("{stem}.vvd"))?)?;
            let vtx = parse_source_vtx(&read(&format!("{stem}.dx90.vtx"))?)?;
            let skeleton = parse_source1_skeleton(&raw)?;
            let materials = parse_source1_materials(&raw)?;
            let gla = humanoid_reference(HumanoidRig::Jk2_72);
            let rig = build_source_rig(
                stem,
                &raw,
                &vvd,
                &vtx,
                &skeleton,
                &materials,
                &gla,
                &SourceRigOptions::default(),
            )?;
            (
                raw,
                rig.glm.clone(),
                gla.as_ref().clone(),
                rig.native_pivots.clone(),
                Some(rig),
            )
        } else {
            let raw = read(&format!("models/players/{model}/model.glm"))?;
            let glm = parse_glm_native(&raw)?;
            (raw, glm, animation.clone(), None, None)
        };
        let bundle = std::fs::read(out.join(format!("{model}.bodyshape")))?;
        let bundle_fingerprint = model_fingerprint(&bundle);
        let grid = BodyShapeGrid::parse(&bundle, &original, model_fingerprint(&raw))?;
        let plan = PoseRetargeter::new(&animation.skeleton, &gla.skeleton)?;
        let stocks: Vec<_> = stocks
            .iter()
            .map(|p| plan.retarget(PoseInput::Ghoul2SkinMatrices(p)))
            .collect::<Result<_, _>>()?;
        let old: Vec<[f32; 3]> = original_pivots.clone().unwrap_or_else(|| {
            gla.skeleton
                .iter()
                .map(|b| std::array::from_fn(|k| b.base_pose[k][3]))
                .collect()
        });
        let baselines: Vec<_> = stocks
            .iter()
            .map(|p| {
                if let Some(pivots) = &original_pivots {
                    rebase_source_ghoul2_pose(&gla, p, pivots)
                } else {
                    Ok(p.clone())
                }
            })
            .collect::<Result<_, _>>()?;
        let skin = if model == "temp" {
            String::new()
        } else {
            String::from_utf8_lossy(&read(&format!(
                "models/players/{model}/model_default.skin"
            ))?)
            .into_owned()
        };
        let off: Vec<_> = skin
            .lines()
            .filter_map(|l| l.split_once(','))
            .filter(|(_, s)| s.trim().eq_ignore_ascii_case("*off"))
            .map(|(n, _)| n.trim().to_ascii_lowercase())
            .collect();
        let visible: Vec<_> = original.lods[0]
            .surfaces
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let h = &original.hierarchy[s.surface_index];
                !h.name.starts_with('*')
                    && h.flags & 3 == 0
                    && !off.contains(&h.name.to_ascii_lowercase())
            })
            .map(|(i, _)| i)
            .collect();
        let nv: usize = visible
            .iter()
            .map(|&i| original.lods[0].surfaces[i].vertices.len())
            .sum();
        let nf: usize = visible
            .iter()
            .map(|&i| original.lods[0].surfaces[i].triangles.len())
            .sum();
        let mut w = BufWriter::new(std::fs::File::create(
            out.join(format!("{model}_poses.bin")),
        )?);
        w.write_all(b"JKPOSES2")?;
        for x in [cases.len(), frames.len(), nv, nf] {
            write_u32(&mut w, x)?;
        }
        for &f in &frames {
            write_u32(&mut w, f)?;
        }
        let mut offset = 0;
        for &si in &visible {
            let s = &original.lods[0].surfaces[si];
            for face in &s.triangles {
                for &i in face {
                    write_u32(&mut w, i as usize + offset)?;
                }
            }
            offset += s.vertices.len();
        }
        for &(name, v) in &cases {
            let params = BodyShapeParams {
                muscle: v[0],
                weight: v[1],
                proportions: v[2],
                age: v[3],
                height: 0.5,
            };
            let mut shaped = original.clone();
            let shaped_pivots =
                grid.apply(&mut shaped, &gla, params, original_pivots.as_deref())?;
            let binding = shaped_pivots
                .as_ref()
                .map(|p| BodyShapeBinding::new(&gla, original_pivots.as_deref(), p))
                .transpose()?;
            let new = shaped_pivots.as_ref().unwrap_or(&old);
            if let Some(rig) = &mut source_rig {
                rig.glm = shaped.clone();
                rig.native_pivots = Some(new.clone());
                let donor = parse_glm_native(&read("models/players/kyle/model.glm")?)?;
                attach_humanoid_bolts_between_rigs(
                    rig,
                    &donor,
                    &humanoid_reference(HumanoidRig::Jka53),
                    &gla,
                )?;
            }
            write_name(&mut w, name)?;
            for x in v {
                w.write_all(&x.to_le_bytes())?;
            }
            let mut worst = 0f32;
            let mut links = 0;
            for (fi, stock) in stocks.iter().enumerate() {
                let pose = if let Some(b) = &binding {
                    b.retarget(&gla, stock)?
                } else {
                    baselines[fi].clone()
                };
                for j in 0..old.len() {
                    if let Some(p) = anatomical_parent(&gla.skeleton, j) {
                        let a = distance(old[j], old[p]);
                        if a < 1e-4 {
                            continue;
                        }
                        let expected = distance(
                            point(&baselines[fi][j], old[j]),
                            point(&baselines[fi][p], old[p]),
                        ) * distance(new[j], new[p])
                            / a;
                        let actual = distance(point(&pose[j], new[j]), point(&pose[p], new[p]));
                        worst = worst.max((actual - expected).abs());
                        links += 1;
                    }
                }
                for &si in &visible {
                    let surface = skin_glm_surface(&shaped.lods[0].surfaces[si], &pose)?;
                    for vertex in surface.vertices {
                        for x in vertex.position {
                            assert!(x.is_finite());
                            w.write_all(&x.to_le_bytes())?;
                        }
                    }
                }
                if let Some(rig) = &source_rig {
                    assert!(model_bolt_matrix(&rig.glm, &gla, &pose, "*r_hand")?.is_some());
                }
            }
            assert!(
                worst < 0.001,
                "{model} {name}: animated joint link error {worst}"
            );
            writeln!(
                diagnostics,
                "{model},{name},{},{links},{worst},{nv},{bundle_fingerprint:016x}",
                frames.len()
            )?;
            eprintln!(
                "{model}/{name}: {} animated links; max proportion error {worst:.6}",
                links
            );
        }
    }
    Ok(())
}
