//! Offline check of the actual stomach detector/masks on installed player meshes.
//! cargo run -p DinurdoJK --example jiggle_probe --profile fast -- <base>
#![allow(dead_code)]
#[path = "../src/cgame/jiggle.rs"]
mod jiggle;

use jiggle::{JiggleProfile, JiggleSolver, JiggleSystem, JiggleTuning};
use jka_assets::{
    body_shape::{anny, BodyShapeParams},
    ghoul2::*,
    pk3::AssetSearchPath,
    skeleton::{humanoid_reference, HumanoidRig},
};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/release/base".into());
    let mut assets = AssetSearchPath::open_game(Path::new(&base), None)?;
    assets.set_allow_asset_overrides(true);
    let mut read = |q: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(assets
            .read(q, 128 * 1024 * 1024)?
            .ok_or_else(|| format!("missing {q}"))?
            .bytes)
    };
    let animation = parse_gla(&read("models/players/_humanoid/_humanoid.gla")?)?;
    for model in ["reelo", "kyle", "jedi_hm", "jedi_hf", "jedi_zf"] {
        let original = parse_glm_native(&read(&format!("models/players/{model}/model.glm"))?)?;
        let reference;
        let gla = if original.num_bones == animation.num_bones() {
            &animation
        } else {
            reference = humanoid_reference(if original.num_bones == 72 {
                HumanoidRig::Jk2_72
            } else {
                HumanoidRig::Jka53
            });
            &reference
        };
        probe(model, &original, gla, None, false)?;
    }
    let stem = "models/player/fwhykay/lollipop_chainsaw/juliet_starling_cine/juliet_starling_american_casual/juliet_starling_american_casual";
    let mdl = read(&format!("{stem}.mdl"))?;
    let vvd = jka_assets::source1_geometry::parse_source_vvd(&read(&format!("{stem}.vvd"))?)?;
    let vtx = jka_assets::source1_geometry::parse_source_vtx(&read(&format!("{stem}.dx90.vtx"))?)?;
    let skeleton = jka_assets::source1::parse_source1_skeleton(&mdl)?;
    let materials = jka_assets::source1::parse_source1_materials(&mdl)?;
    let gla = humanoid_reference(HumanoidRig::Jk2_72);
    let rig = jka_assets::source1_rig::build_source_rig(
        &format!("{stem}.mdl"),
        &mdl,
        &vvd,
        &vtx,
        &skeleton,
        &materials,
        &gla,
        &jka_assets::source1_rig::SourceRigOptions::default(),
    )?;
    probe(
        "source_juliet",
        &rig.glm,
        &gla,
        rig.native_pivots.as_deref(),
        true,
    )?;
    Ok(())
}

fn probe(
    model: &str,
    original: &GlmModel,
    gla: &GlaAnimation,
    native: Option<&[[f32; 3]]>,
    source: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for pregnant in [false, true] {
        let mut glm = original.clone();
        let pivots = if pregnant {
            anny::apply(
                &mut glm,
                gla,
                BodyShapeParams::default(),
                anny::Details::parse("stomach-pregnant-incr=1")?,
                native,
            )?
        } else {
            native.map(|v| v.to_vec())
        };
        let profile = if source {
            JiggleProfile::auto_source_humanoid(&glm, gla, pivots.as_deref())
        } else {
            JiggleProfile::auto_humanoid(&glm, gla, pivots.as_deref())
        }?;
        let Some(profile) = profile else {
            println!("{model} pregnant={pregnant} NO PROFILE");
            continue;
        };
        let offsets = profile.gpu_offsets(
            &vec![[1.0; 3]; profile.region_count()],
            JiggleTuning {
                stomach_strength: 1.0,
                breast_strength: 0.0,
                glute_strength: 0.0,
                ..Default::default()
            },
        );
        let stomach = offsets.iter().position(|offset| offset[0] > 0.0);
        let mut selected = Vec::new();
        if let Some(stomach) = stomach {
            for surface in &glm.lods[0].surfaces {
                for (i, vertex) in surface.vertices.iter().enumerate() {
                    let (region, weight, _) =
                        profile.gpu_vertex_binding(0, surface.surface_index, i);
                    if region as usize == stomach && weight > 0.01 {
                        selected.push((vertex.position[2], weight));
                    }
                }
            }
        }
        let min_z = selected.iter().map(|v| v.0).fold(f32::INFINITY, f32::min);
        let max_z = selected
            .iter()
            .map(|v| v.0)
            .fold(f32::NEG_INFINITY, f32::max);
        let peak = selected.iter().map(|v| v.1).fold(0.0, f32::max);
        let expected = pregnant || model == "reelo";
        // The neutral Source donor may already have authored fullness;
        // only the known flat stock meshes are negative fixtures.
        if !source || pregnant {
            assert_eq!(stomach.is_some(), expected, "{model} pregnant={pregnant}");
        }
        if expected {
            let lumbar = gla
                .skeleton
                .iter()
                .position(|b| b.name == "upper_lumbar")
                .unwrap();
            let lumbar_z = pivots
                .as_ref()
                .map_or(gla.skeleton[lumbar].base_pose[2][3], |p| p[lumbar][2]);
            assert!(max_z > lumbar_z + 2.0, "{model}: upper abdomen missing");
            assert!(peak > 0.35, "{model}: motion almost muted");
            assert!(profile.gpu_supported());
            let identity = [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ];
            let pose = vec![identity; gla.num_bones()];
            let axis = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            for solver in [JiggleSolver::KawaiiPhysics, JiggleSolver::JigglePhysics] {
                let mut system = JiggleSystem::default();
                system.set_config(
                    true,
                    60,
                    4,
                    JiggleTuning {
                        solver,
                        stomach_strength: 1.0,
                        ..Default::default()
                    },
                );
                for (time, travel) in [(0, 0.0), (17, 0.0), (34, 1.0)] {
                    system.simulate_snapshot(
                        1,
                        model,
                        &profile,
                        &pose,
                        axis,
                        [travel, 0.0, travel],
                        time,
                    );
                }
                let snapshot =
                    system.simulate_snapshot(1, model, &profile, &pose, axis, [3.0, 0.0, 3.0], 51);
                let motion = snapshot.offsets[stomach.unwrap()];
                assert!(motion.iter().all(|v| v.is_finite()));
                assert!(
                    motion.iter().map(|v| v * v).sum::<f32>().sqrt() > 0.01,
                    "{model}: {solver:?} did not move stomach"
                );
            }
        }
        println!("{model} pregnant={pregnant} stomach={stomach:?} gpu={} vertices={} z={min_z:.2}..{max_z:.2} peak={peak:.3}",
                profile.gpu_supported(), selected.len());
    }
    Ok(())
}
