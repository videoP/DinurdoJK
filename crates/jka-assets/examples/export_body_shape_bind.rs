//! Capture the exact Source -> Ghoul2 neutral mesh for offline Anny authoring.
//! Usage: export_body_shape_bind <base> <mdl-qpath> <output.bind> [scale]
//! Defaults match SourceRigOptions. Non-default bodygroup/rebind descriptors
//! require the same options here; the runtime bind hash rejects mismatches.
use jka_assets::{
    body_shape::{binding_fingerprint, model_fingerprint},
    pk3::AssetSearchPath,
    skeleton::{humanoid_reference, HumanoidRig},
    source1::{parse_source1_materials, parse_source1_skeleton},
    source1_geometry::{parse_source_vtx, parse_source_vvd},
    source1_rig::{build_source_rig, SourceRigOptions},
};
use std::{io::Write, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err(
            "usage: export_body_shape_bind <base> <mdl-qpath> <output.bind> [scale]".into(),
        );
    }
    let mut assets = AssetSearchPath::open_game(Path::new(&args[0]), None)?;
    assets.set_allow_asset_overrides(true);
    let stem = args[1].strip_suffix(".mdl").ok_or("expected .mdl")?;
    let mut read = |q: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(assets
            .read(q, 64 * 1024 * 1024)?
            .ok_or_else(|| format!("missing {q}"))?
            .bytes)
    };
    let mdl = read(&args[1])?;
    let vvd = parse_source_vvd(&read(&format!("{stem}.vvd"))?)?;
    let vtx = parse_source_vtx(&read(&format!("{stem}.dx90.vtx"))?)?;
    let skeleton = parse_source1_skeleton(&mdl)?;
    let materials = parse_source1_materials(&mdl)?;
    let gla = humanoid_reference(HumanoidRig::Jk2_72);
    let mut options = SourceRigOptions::default();
    if let Some(scale) = args.get(3) {
        options.scale = Some(scale.parse()?);
    }
    let rig = build_source_rig(
        &args[1], &mdl, &vvd, &vtx, &skeleton, &materials, &gla, &options,
    )?;
    let mut out = std::fs::File::create(&args[2])?;
    out.write_all(b"JKSHBND2")?;
    out.write_all(&model_fingerprint(&mdl).to_le_bytes())?;
    out.write_all(&binding_fingerprint(&rig.glm).to_le_bytes())?;
    out.write_all(&(gla.skeleton.len() as u32).to_le_bytes())?;
    out.write_all(&(rig.glm.lods.len() as u32).to_le_bytes())?;
    for (i, bone) in gla.skeleton.iter().enumerate() {
        out.write_all(&(bone.name.len() as u32).to_le_bytes())?;
        out.write_all(bone.name.as_bytes())?;
        for k in 0..3 {
            let value = rig
                .native_pivots
                .as_ref()
                .map_or(bone.base_pose[k][3], |p| p[i][k]);
            out.write_all(&value.to_le_bytes())?;
        }
    }
    for lod in &rig.glm.lods {
        out.write_all(&(lod.surfaces.len() as u32).to_le_bytes())?;
        for surface in &lod.surfaces {
            let hierarchy = &rig.glm.hierarchy[surface.surface_index];
            out.write_all(&(surface.surface_index as u32).to_le_bytes())?;
            out.write_all(&hierarchy.flags.to_le_bytes())?;
            out.write_all(&(hierarchy.name.len() as u32).to_le_bytes())?;
            out.write_all(hierarchy.name.as_bytes())?;
            out.write_all(&(surface.vertices.len() as u32).to_le_bytes())?;
            out.write_all(&(surface.triangles.len() as u32).to_le_bytes())?;
            for vertex in &surface.vertices {
                for x in vertex.position.into_iter().chain(vertex.normal) {
                    out.write_all(&x.to_le_bytes())?;
                }
                out.write_all(&(vertex.weights.len() as u32).to_le_bytes())?;
                for weight in &vertex.weights {
                    out.write_all(
                        &(surface.bone_references[weight.local_bone_index] as u32).to_le_bytes(),
                    )?;
                    out.write_all(&weight.weight.to_le_bytes())?;
                }
            }
            for face in &surface.triangles {
                for index in face {
                    out.write_all(&index.to_le_bytes())?;
                }
            }
        }
    }
    eprintln!(
        "Captured {} bones, {} LODs, bind {:016x}",
        gla.skeleton.len(),
        rig.glm.lods.len(),
        binding_fingerprint(&rig.glm)
    );
    Ok(())
}
