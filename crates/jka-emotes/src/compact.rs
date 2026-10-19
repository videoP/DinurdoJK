//! Library maintenance: repack a directory of bundles without resampling poses
//! or re-encoding audio.
use crate::compiler::{ground_mesh, target_from_model, DEFAULT_TARGET_MODEL};
use crate::Bundle;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

pub fn compact_library(
    input: &Path,
    output: &Path,
    recalibrate_base: Option<&Path>,
) -> Result<(), String> {
    use std::io::Read;
    if output.exists() {
        return Err(format!("output already exists: {}", output.display()));
    }
    let mut paths = fs::read_dir(input)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    paths.sort_by_key(|p| p.file_name());
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let mut count = 0u64;
    let mut before = 0u64;
    let mut after = 0u64;
    let mut removed = 0u64;
    let mut normalized = 0u64;
    let mut ground_cache = BTreeMap::<String, Value>::new();
    for entry in paths {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("djk_fnemote"))
        {
            continue;
        }
        let bundle = Bundle::open(&path)?;
        let mut root = bundle.manifest.clone();
        let file = fs::File::open(&path).map_err(|e| e.to_string())?;
        before += file.metadata().map_err(|e| e.to_string())?.len();
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let mut members = BTreeMap::<String, Vec<u8>>::new();
        let mut aliases = BTreeMap::<String, String>::new();
        for i in 0..archive.len() {
            let mut member = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = member.name().to_owned();
            if name == "manifest.json" {
                continue;
            }
            let mut bytes = Vec::with_capacity(member.size() as usize);
            member.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            if name.starts_with("audio/") && bytes.starts_with(b"fLaC") {
                crate::validate_audio(&bytes)?;
                // Locate the first frame after all metadata blocks. Normalize only
                // fixed-block streams, preserving variable-block source assets.
                let mut at = 4usize;
                loop {
                    let last = bytes[at] & 0x80 != 0;
                    let size = ((bytes[at + 1] as usize) << 16)
                        | ((bytes[at + 2] as usize) << 8)
                        | bytes[at + 3] as usize;
                    at = at.checked_add(4 + size).ok_or("FLAC metadata overflow")?;
                    if at + 4 > bytes.len() {
                        return Err(format!("truncated FLAC metadata in {name}"));
                    }
                    if last {
                        break;
                    }
                }
                if bytes[at] == 0xff && bytes[at + 1] == 0xf8 && bytes[8..10] != bytes[10..12] {
                    let block = [bytes[10], bytes[11]];
                    bytes[8..10].copy_from_slice(&block);
                    normalized += 1;
                }
                if let Some((existing, _)) = members
                    .iter()
                    .find(|(n, b)| n.starts_with("audio/") && **b == bytes)
                {
                    aliases.insert(name, existing.clone());
                    removed += 1;
                    continue;
                }
            }
            members.insert(name, bytes);
        }
        fn replace(value: &mut Value, aliases: &BTreeMap<String, String>) {
            match value {
                Value::String(s) => {
                    if let Some(alias) = aliases.get(s) {
                        *s = alias.clone();
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        replace(value, aliases);
                    }
                }
                Value::Object(values) => {
                    for value in values.values_mut() {
                        replace(value, aliases);
                    }
                }
                _ => {}
            }
        }
        replace(&mut root, &aliases);
        if let Some(base) = recalibrate_base {
            let mut recipe: Value = serde_json::from_slice(
                members
                    .get("metadata/recipe.json")
                    .ok_or("bundle lacks recipe for recalibration")?,
            )
            .map_err(|e| e.to_string())?;
            let old_floor = recipe["animation"]["retarget"]["ground_mesh"]["floor_z"]
                .as_f64()
                .ok_or("bundle lacks ground calibration")?;
            let gla = jka_assets::ghoul2::parse_gla(
                members.get("animation.gla").ok_or("bundle lacks GLA")?,
            )?;
            let model = recipe["animation"]["target_model"]
                .as_str()
                .unwrap_or(DEFAULT_TARGET_MODEL);
            let ground = if let Some(ground) = ground_cache.get(model) {
                ground.clone()
            } else {
                let reference = jka_assets::ghoul2::parse_gla(&target_from_model(base, model)?)?;
                let ground = ground_mesh(base, &reference, model)?;
                ground_cache.insert(model.to_owned(), ground.clone());
                ground
            };
            let shift = (ground["floor_z"]
                .as_f64()
                .ok_or("invalid new ground calibration")?
                - old_floor) as f32;
            if !shift.is_finite() || shift.abs() > 32.0 {
                return Err("recalibration exceeds 32 units".into());
            }
            if shift.abs() > 0.0001 {
                let pose = members
                    .get_mut("animation.pose")
                    .ok_or("bundle lacks baked pose")?;
                for m in pose[16..].chunks_exact_mut(48) {
                    let z = f32::from_le_bytes(m[44..48].try_into().unwrap()) + shift;
                    m[44..48].copy_from_slice(&z.to_le_bytes());
                }
                let mut locals = Vec::new();
                for frame in 0..gla.num_frames {
                    for (bone, reference) in gla.skeleton.iter().enumerate() {
                        let m = gla.decompress_bone(frame, bone)?;
                        let rotation = glam::Mat3::from_cols_array(&[
                            m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2],
                            m[2][2],
                        ]);
                        locals.push((
                            glam::Quat::from_mat3(&rotation).normalize(),
                            glam::Vec3::new(
                                m[0][3],
                                m[1][3],
                                m[2][3] + if reference.parent < 0 { shift } else { 0.0 },
                            ),
                        ));
                    }
                }
                members.insert(
                    "animation.gla".into(),
                    crate::retarget::encode(&gla, gla.num_frames, &locals)?,
                );
                fn report_shift(report: &mut Value, shift: f32) {
                    if let Some(z) = report["ground_shift_z"].as_f64() {
                        report["ground_shift_z"] = json!(z + shift as f64);
                    }
                    for variant in report["variants"].as_array_mut().into_iter().flatten() {
                        report_shift(&mut variant["conversion"], shift);
                    }
                }
                report_shift(&mut root["conversion"], shift);
                root["ground_recalibration_z"] = json!(shift);
            }
            recipe["animation"]["retarget"]["ground_mesh"] = ground;
            recipe["batch"]["policy_version"] = json!(8);
            members.insert(
                "metadata/recipe.json".into(),
                serde_json::to_vec_pretty(&recipe).map_err(|e| e.to_string())?,
            );
            let recipes = output.join("recipes");
            fs::create_dir_all(&recipes).map_err(|e| e.to_string())?;
            fs::write(
                recipes.join(entry.file_name()).with_extension("json"),
                serde_json::to_vec_pretty(&recipe).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        let destination = output.join(entry.file_name());
        crate::write_bundle(&destination, &root, &members)?;
        after += fs::metadata(destination).map_err(|e| e.to_string())?.len();
        count += 1;
        if count % 100 == 0 {
            println!("Compacted {count} bundles");
        }
    }
    fn copy_metadata(input: &Path, output: &Path, depth: usize) -> Result<(), String> {
        if depth > 8 {
            return Err("library metadata nesting exceeds 8".into());
        }
        for entry in fs::read_dir(input).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let path = entry.path();
            let dest = output.join(entry.file_name());
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
                copy_metadata(&path, &dest, depth + 1)?;
            } else if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("djk_fnemote"))
            {
                if !dest.exists() {
                    fs::copy(path, dest).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }
    copy_metadata(input, output, 0)?;
    println!(
        "{}",
        json!({"bundles":count,"bytes_before":before,"bytes_after":after,"duplicate_tracks_removed":removed,"flac_headers_normalized":normalized})
    );
    Ok(())
}
