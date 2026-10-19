//! Rebinds an exported Unreal character onto `_humanoid`. Needs local export
//! data, so it only runs with `DJK_UE_CHARACTER=<mesh.skeletal.json;...>`.
use jka_assets::skeleton::{humanoid_reference, HumanoidRig};
use jka_assets::source1_rig::{build_rig, SourceRigOptions};
use jka_ue_assets::character::{character_rig_input, CharacterPart};

#[test]
fn exported_character_rebinds_onto_humanoid() {
    let Ok(list) = std::env::var("DJK_UE_CHARACTER") else {
        return;
    };
    let meshes: Vec<(String, Vec<u8>, Vec<String>)> = list
        .split(';')
        .enumerate()
        .map(|(i, path)| {
            let bytes = std::fs::read(path).unwrap();
            let sections = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["sections"]
                .as_array()
                .unwrap()
                .len();
            (
                format!("part{i}"),
                bytes,
                (0..sections).map(|s| format!("part{i}_s{s}")).collect(),
            )
        })
        .collect();
    let parts: Vec<CharacterPart> = meshes
        .iter()
        .map(|(name, mesh, shaders)| CharacterPart {
            name,
            mesh,
            shaders,
            attach: None,
        })
        .collect();
    let (skeleton, geometry) = character_rig_input("test", &parts).unwrap();
    let gla = humanoid_reference(HumanoidRig::Jk2_72);
    let rig = build_rig(
        "test",
        &skeleton,
        &geometry,
        &gla,
        &SourceRigOptions::default(),
    )
    .unwrap();
    let d = &rig.diagnostics;
    println!(
        "invbind={:.3} bones={} mapped={} inherited={} scale={:.4} fitRMS={:.2} yaw={:.1} worst={:?} reshaped={:.2} truncations={} bounds={:?} lods={:?}",
        d.max_inverse_bind_error, d.source_bones, d.mapped_bones, d.inherited_bones.len(), d.scale, d.fit_rms, d.fit_yaw_degrees, d.fit_worst,
        d.max_rebind_displacement, d.weight_truncations, d.rest_bounds, d.lods
    );
    assert!(d.fit_rms < 10.0, "skeleton fit too loose");
}
