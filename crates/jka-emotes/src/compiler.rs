//! Recipe compiler: one source export (PSA or montage, sound graphs, metadata)
//! to one self-contained `.djk_fnemote` bundle.
//!
//! The recipe format is documented in `docs/recipes.md`; this module is its
//! reference implementation. Everything here is also what the batch tooling in
//! `djk-fnemote` runs per item, so the same checks apply to hand-written recipes.
use crate::{audio_mix, montage, MAX_MEMBER_BYTES};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Stock Jedi Academy model whose skeleton and mesh calibrate new emotes when a
/// recipe does not name a `target_model`.
pub const DEFAULT_TARGET_MODEL: &str = "models/players/kyle/model.glm";

pub fn read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit as u64 {
        return Err(format!("{} exceeds size limit", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("source file exceeds limit".into());
    }
    Ok(bytes)
}
/// Repack an existing library without resampling animations or re-encoding audio.
/// Fixed-block FLAC headers are normalized and identical cooked tracks share a member.
pub fn target_from_model(base: &Path, model: &str) -> Result<Vec<u8>, String> {
    let model = crate::manifest::qpath(model)?;
    if !model.to_ascii_lowercase().ends_with(".glm") {
        return Err("target_model must be a GLM qpath".into());
    }
    let mut assets = jka_assets::pk3::AssetSearchPath::open(base).map_err(|e| e.to_string())?;
    let mesh = assets
        .read(&model, MAX_MEMBER_BYTES)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("missing target model {model}"))?;
    let name = jka_assets::ghoul2::glm_animation_name(&mesh.bytes)?;
    // Bake the full digit rig once. Runtime binds it to the selected native
    // mesh (53 or 72 bones), rather than discarding joints at compile time.
    if name
        .replace('\\', "/")
        .to_ascii_lowercase()
        .contains("players/_humanoid/")
    {
        return Ok(include_bytes!("../../jka-assets/src/skeleton/jk2-reference.gla").to_vec());
    }
    let qpath = crate::manifest::qpath(&format!("{}.gla", name.trim_end_matches(".gla")))?;
    assets
        .read(&qpath, MAX_MEMBER_BYTES)
        .map_err(|e| e.to_string())?
        .map(|a| a.bytes)
        .ok_or_else(|| format!("{model} references missing skeleton {qpath}"))
}
pub fn text<'a>(root: &'a Value, key: &str) -> Result<&'a str, String> {
    root.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("recipe missing {key}"))
}
pub fn relative(root: &Path, value: &str) -> Result<PathBuf, String> {
    let normalized = crate::manifest::qpath(value)?;
    let full = root
        .join(normalized)
        .canonicalize()
        .map_err(|e| format!("{value}: {e}"))?;
    if !full.starts_with(root) {
        return Err(format!("source member escapes source_root: {value}"));
    }
    Ok(full)
}
pub fn compile(recipe_file: &Path, output: &Path) -> Result<(), String> {
    let recipe: Value = serde_json::from_slice(&read(recipe_file, crate::MAX_MANIFEST_BYTES)?)
        .map_err(|e| e.to_string())?;
    compile_recipe(
        &recipe,
        recipe_file.parent().unwrap_or(Path::new(".")),
        output,
        None,
    )
}
pub fn ground_mesh(
    base: &Path,
    target: &jka_assets::ghoul2::GlaAnimation,
    model_qpath: &str,
) -> Result<Value, String> {
    let mut assets = jka_assets::pk3::AssetSearchPath::open(base).map_err(|e| e.to_string())?;
    let Some(asset) = assets
        .read(&crate::manifest::qpath(model_qpath)?, MAX_MEMBER_BYTES)
        .map_err(|e| e.to_string())?
    else {
        return Ok(Value::Null);
    };
    let model = jka_assets::ghoul2::parse_glm_native(&asset.bytes)?;
    let mesh_rig = match model.num_bones {
        72 => jka_assets::skeleton::humanoid_reference(jka_assets::skeleton::HumanoidRig::Jk2_72),
        53 => jka_assets::skeleton::humanoid_reference(jka_assets::skeleton::HumanoidRig::Jka53),
        _ => return Ok(Value::Null),
    };
    let bone_map = mesh_rig
        .skeleton
        .iter()
        .enumerate()
        .map(|(i, bone)| {
            let mut ancestor = Some(i);
            while let Some(a) = ancestor {
                if let Some(t) = target
                    .skeleton
                    .iter()
                    .position(|t| t.name.eq_ignore_ascii_case(&mesh_rig.skeleton[a].name))
                {
                    return Ok(t);
                }
                ancestor = jka_assets::skeleton::anatomical_parent(&mesh_rig.skeleton, a);
            }
            Err(format!("grounding bone missing: {}", bone.name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut points = Vec::new();
    let mut floor = f32::INFINITY;
    for surface in &model.lods[0].surfaces {
        let name = &model.hierarchy[surface.surface_index].name;
        if name.contains("_off") || name.contains("cap") {
            continue;
        }
        for vertex in &surface.vertices {
            let weights: Vec<_> = vertex
                .weights
                .iter()
                .map(|w| {
                    (
                        bone_map[surface.bone_references[w.local_bone_index]],
                        w.weight,
                    )
                })
                .collect();
            floor = floor.min(vertex.position[2]);
            points.push(json!({"position":vertex.position,"weights":weights}));
        }
    }
    if points.is_empty() {
        return Ok(Value::Null);
    }
    let gameplay = assets
        .read(
            &format!("{}.gla", model.anim_name.trim_end_matches(".gla")),
            MAX_MEMBER_BYTES,
        )
        .map_err(|e| e.to_string())?
        .ok_or("missing grounding reference animation")?;
    let gameplay = jka_assets::ghoul2::parse_gla(&gameplay.bytes)?;
    let standing = assets
        .read(
            &format!(
                "{}/animation.cfg",
                model
                    .anim_name
                    .rsplit_once('/')
                    .map(|a| a.0)
                    .unwrap_or("models/players/_humanoid")
            ),
            MAX_MEMBER_BYTES,
        )
        .map_err(|e| e.to_string())?
        .and_then(|a| String::from_utf8(a.bytes).ok())
        .and_then(|cfg| {
            cfg.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                if fields.next() == Some("BOTH_STAND1") {
                    fields.next()?.parse::<usize>().ok()
                } else {
                    None
                }
            })
        })
        .filter(|n| *n < gameplay.num_frames)
        .unwrap_or(0);
    let reference = gameplay.compose_frame(standing)?;
    let reference =
        jka_assets::skeleton::PoseRetargeter::new(&gameplay.skeleton, &target.skeleton)?.retarget(
            jka_assets::skeleton::PoseInput::Ghoul2SkinMatrices(&reference),
        )?;
    let ground = points
        .iter()
        .map(|point| {
            let p = &point["position"];
            point["weights"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| {
                    let row = &reference[w[0].as_u64().unwrap() as usize][2];
                    w[1].as_f64().unwrap() as f32
                        * (row[0] * p[0].as_f64().unwrap() as f32
                            + row[1] * p[1].as_f64().unwrap() as f32
                            + row[2] * p[2].as_f64().unwrap() as f32
                            + row[3])
                })
                .sum::<f32>()
        })
        .fold(f32::INFINITY, f32::min);
    Ok(
        json!({"model":model_qpath,"bind_floor":floor,"floor_z":ground,"reference_frame":standing,"points":points}),
    )
}
pub fn compile_recipe(
    recipe: &Value,
    recipe_dir: &Path,
    output: &Path,
    cached_target: Option<&jka_assets::ghoul2::GlaAnimation>,
) -> Result<(), String> {
    if !output
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("djk_fnemote"))
    {
        return Err("output must have .djk_fnemote extension".into());
    }
    if output.exists() {
        return Err(format!("output already exists: {}", output.display()));
    }
    let source = recipe_dir
        .join(text(recipe, "source_root")?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let animation = recipe.get("animation").ok_or("recipe missing animation")?;
    let mut members: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let (gla_bytes, first, end, fps, report) = if animation["variants"].is_array() {
        let loaded;
        let target = if let Some(target) = cached_target {
            target
        } else {
            loaded = jka_assets::ghoul2::parse_gla(&target_from_model(
                &recipe_dir.join(text(animation, "target_base")?),
                animation["target_model"]
                    .as_str()
                    .unwrap_or(DEFAULT_TARGET_MODEL),
            )?)?;
            &loaded
        };
        let mut settings = animation.clone();
        if settings["retarget"]["ground_mesh"].is_null()
            && settings["retarget"]["ground_contact"] != false
        {
            if let Some(base) = animation["target_base"].as_str() {
                if !settings["retarget"].is_object() {
                    settings["retarget"] = json!({});
                }
                settings["retarget"]["ground_mesh"] = ground_mesh(
                    &recipe_dir.join(base),
                    target,
                    animation["target_model"]
                        .as_str()
                        .unwrap_or(DEFAULT_TARGET_MODEL),
                )?;
            }
        }
        let (gla, pose, report) = montage::bake(&source, &settings, target)?;
        members.insert("animation.pose".into(), pose);
        let end = report["variants"][0]["end_frame"]
            .as_i64()
            .ok_or("missing variant end")? as i32;
        (gla, 0, end, 30.0, report)
    } else {
        match (animation.get("psa"), animation.get("gla")) {
            (Some(psa), None) => {
                let psa_path = relative(&source, psa.as_str().ok_or("psa must be a path")?)?;
                let source_bytes = read(&psa_path, MAX_MEMBER_BYTES)?;
                let psa = crate::psa::Psa::load(&psa_path)?;
                let loaded_target;
                let target = if let Some(target) = cached_target {
                    target
                } else {
                    let bytes = match (animation.get("target_base"), animation.get("target_gla")) {
                        (Some(base), None) => target_from_model(
                            &recipe_dir.join(base.as_str().ok_or("invalid target_base")?),
                            animation["target_model"]
                                .as_str()
                                .unwrap_or(DEFAULT_TARGET_MODEL),
                        )?,
                        (None, Some(gla)) => read(
                            &recipe_dir.join(gla.as_str().ok_or("invalid target_gla")?),
                            MAX_MEMBER_BYTES,
                        )?,
                        _ => {
                            return Err(
                                "PSA recipe needs exactly one of target_base or target_gla".into()
                            )
                        }
                    };
                    loaded_target = jka_assets::ghoul2::parse_gla(&bytes)?;
                    &loaded_target
                };
                let mut settings = animation["retarget"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                if !settings.contains_key("ground_mesh")
                    && settings
                        .get("ground_contact")
                        .and_then(Value::as_bool)
                        .unwrap_or(true)
                {
                    if let Some(base) = animation["target_base"].as_str() {
                        settings.insert(
                            "ground_mesh".into(),
                            ground_mesh(
                                &recipe_dir.join(base),
                                target,
                                animation["target_model"]
                                    .as_str()
                                    .unwrap_or(DEFAULT_TARGET_MODEL),
                            )?,
                        );
                    }
                }
                let (gla, pose, report) =
                    crate::retarget::bake(&psa, target, &Value::Object(settings))?;
                members.insert("animation.pose".into(), pose);
                let parsed = jka_assets::ghoul2::parse_gla(&gla)?;
                if recipe["include_source_psa"].as_bool().unwrap_or(false) {
                    members.insert("source/animation.psa".into(), source_bytes);
                }
                (gla, 0, parsed.num_frames as i32, psa.fps, report)
            }
            (None, Some(gla)) => {
                let bytes = read(
                    &relative(&source, gla.as_str().ok_or("gla must be a path")?)?,
                    MAX_MEMBER_BYTES,
                )?;
                let parsed = jka_assets::ghoul2::parse_gla(&bytes)?;
                let first = animation
                    .get("first_frame")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let end = animation
                    .get("end_frame")
                    .and_then(Value::as_i64)
                    .unwrap_or(parsed.num_frames as i64);
                let first = i32::try_from(first).map_err(|_| "first_frame outside i32")?;
                let end = i32::try_from(end).map_err(|_| "end_frame outside i32")?;
                let fps = animation
                    .get("fps")
                    .and_then(Value::as_f64)
                    .ok_or("GLA recipe needs fps")? as f32;
                (bytes, first, end, fps, json!({"method":"prebaked-gla"}))
            }
            _ => return Err("animation must select exactly one of psa or gla".into()),
        }
    };
    members.insert("animation.gla".into(), gla_bytes);
    let mut audio = recipe.get("audio").cloned().unwrap_or(json!([]));
    let mut variant_ranges = Vec::new();
    if let Some(variants) = report["variants"].as_array() {
        let mut combined = Vec::new();
        for variant in variants {
            let start = combined.len();
            combined.extend(variant["audio"].as_array().cloned().unwrap_or_default());
            variant_ranges.push((start, combined.len()));
        }
        audio = json!(combined);
    }
    let entries = audio.as_array_mut().ok_or("audio must be an array")?;
    if cached_target.is_some() && variant_ranges.is_empty() {
        // Montage stops may exceed a standalone child clip. Its playback ends
        // at the PSA boundary; a cue beginning beyond that boundary is omitted.
        let duration = (((end - first) as f64 * 1000.0 / f64::from(fps)).ceil()) as u64;
        entries.retain(|e| {
            (e["wav"].is_null() && e["mix"].is_null())
                || e["at_ms"].as_u64().unwrap_or(u64::MAX) < duration
        });
        for event in entries.iter_mut() {
            if event["wav"].is_null()
                && event["mix"].is_null()
                && event["at_ms"].as_u64().is_some_and(|t| t > duration)
            {
                event["at_ms"] = json!(duration);
            }
        }
    }
    let mut audio_files: BTreeMap<String, String> = BTreeMap::new();
    for event in entries {
        if let Some(mix) = event.get("mix") {
            let member = format!("audio/mix-{:04}.flac", members.len());
            let bytes = audio_mix::render(&source, mix)?;
            crate::validate_wav(&bytes)?;
            if let Some(start) = event["loop_start_seconds"].as_f64() {
                let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
                event["loop_start_sample"] = json!((start * rate as f64).round() as u64);
                event.as_object_mut().unwrap().remove("loop_start_seconds");
            }
            event["silent_pcm"] = json!(audio_mix::is_silent(&bytes)?);
            let compressed = audio_mix::compress(&bytes)?;
            let duplicate = members
                .iter()
                .find(|(name, bytes)| name.starts_with("audio/") && **bytes == compressed)
                .map(|(name, _)| name.clone());
            let member = duplicate.unwrap_or_else(|| {
                members.insert(member.clone(), compressed);
                member
            });
            event.as_object_mut().unwrap().remove("mix");
            event["wav"] = Value::String(member);
            continue;
        }
        if let Some(wav) = event.get("wav").and_then(Value::as_str) {
            let normalized = crate::manifest::qpath(wav)?;
            let next = audio_files.len();
            let member = if let Some(name) = audio_files.get(&normalized) {
                name.clone()
            } else {
                let member = format!("audio/{next:04}.wav");
                let bytes = read(&relative(&source, &normalized)?, MAX_MEMBER_BYTES)?;
                crate::validate_wav(&bytes)?;
                members.insert(member.clone(), bytes);
                audio_files.insert(normalized, member.clone());
                member
            };
            event["wav"] = Value::String(member);
        }
    }
    if recipe["include_compressed_sources"]
        .as_bool()
        .unwrap_or(false)
    {
        let source_dir = source.join("Audio/Source");
        if source_dir.is_dir() {
            for entry in fs::read_dir(source_dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_file() {
                    let member = format!("source/audio/{}", entry.file_name().to_string_lossy());
                    crate::manifest::qpath(&member)?;
                    members.insert(member, read(&entry.path(), MAX_MEMBER_BYTES)?);
                }
            }
        }
    }
    if recipe["include_all_audio"].as_bool().unwrap_or(false) && source.join("Audio").is_dir() {
        let mut files = fs::read_dir(source.join("Audio"))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                || !entry
                    .path()
                    .extension()
                    .is_some_and(|v| v.eq_ignore_ascii_case("wav"))
            {
                continue;
            }
            let normalized = format!("Audio/{}", entry.file_name().to_string_lossy());
            if audio_files.contains_key(&normalized) {
                continue;
            }
            let member = format!("audio/{:04}.wav", audio_files.len());
            let bytes = read(&relative(&source, &normalized)?, MAX_MEMBER_BYTES)?;
            // Unscheduled source WAVs are retained even if the playback decoder
            // cannot accept their format/size. Scheduled WAVs above are validated.
            members.insert(member.clone(), bytes);
            audio_files.insert(normalized, member);
        }
    }
    let mut manifest = json!({"format":"djk_fnemote","version":1,"target_model":animation["target_model"].as_str().unwrap_or(DEFAULT_TARGET_MODEL),"emotes":[{
        "id":text(recipe,"id")?,"name":text(recipe,"name")?,"gla":"animation.gla",
        "first_frame":first,"end_frame":end,"fps":fps,"loop":recipe.get("loop").and_then(Value::as_bool).unwrap_or(false),"audio":audio
    }],"conversion":report,"source_audio":audio_files});
    if !variant_ranges.is_empty() {
        let mut variants = report["variants"].as_array().unwrap().clone();
        for (variant, (start, end)) in variants.iter_mut().zip(variant_ranges) {
            variant["audio"] = json!(&audio.as_array().unwrap()[start..end]);
            let cues = variant["audio"].as_array().unwrap();
            if !cues.is_empty() && cues.iter().all(|e| e["silent_pcm"] == true) {
                variant["audio_status"] = json!("silent_mix");
                variant["audio_detail"]=json!("Sound events resolve, but the rendered PCM is silent. Inspect source samples and graph gain/timing; this is not evidence of a missing download.");
            } else if cues.iter().any(|e| e["lane"] == "music")
                && cues
                    .iter()
                    .filter(|e| e["lane"] == "music")
                    .all(|e| e["silent_pcm"] == true)
            {
                variant["audio_status"] = json!("partial");
                variant["audio_detail"]=json!("Music events render to silent PCM; only other sound effects may be audible. Inspect source samples and graph gain/timing.");
            }
        }
        let first = &variants[0];
        for key in [
            "first_frame",
            "end_frame",
            "loop",
            "loop_start_frame",
            "audio",
            "variant_name",
            "audio_status",
            "audio_detail",
        ] {
            manifest["emotes"][0][key] = first[key].clone();
        }
        manifest["emotes"][0]["variants"] = json!(variants);
    }
    if members.contains_key("animation.pose") {
        manifest["emotes"][0]["pose"] = json!("animation.pose");
    }
    crate::manifest::parse_manifest(&manifest)?;
    members.insert(
        "metadata/recipe.json".into(),
        serde_json::to_vec_pretty(&recipe).map_err(|e| e.to_string())?,
    );
    // Keep all extracted JSONs (montage tracks, notifies, audio graphs and reports)
    // in the same file. Unselected random WAV alternatives are intentionally excluded.
    collect_metadata(&source, &source, &mut members, 0)?;
    crate::write_bundle(output, &manifest, &members)?;
    if cached_target.is_none() {
        println!(
            "Compiled {}: {} frames at {} fps, {} audio clips, {} members",
            output.display(),
            end - first,
            fps,
            audio_files.len(),
            members.len() + 1
        );
    }
    Ok(())
}
pub fn collect_metadata(
    root: &Path,
    dir: &Path,
    members: &mut BTreeMap<String, Vec<u8>>,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("source metadata directory nesting exceeds 16".into());
    }
    let mut entries = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            collect_metadata(root, &path, members, depth + 1)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let name = format!("metadata/source/{relative}");
            members.insert(name, read(&path, MAX_MEMBER_BYTES)?);
            if members.len() > 4095 {
                return Err("too many source metadata members".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod target_tests {
    use super::*;
    #[test]
    #[ignore = "requires DJK_EMOTE_BASE with stock Kyle/Jan assets"]
    fn chosen_glm_supplies_its_skeleton_reference_and_grounding_geometry() {
        let base = PathBuf::from(std::env::var("DJK_EMOTE_BASE").unwrap());
        let jan = "models/players/jan/model.glm";
        let skeleton =
            jka_assets::ghoul2::parse_gla(&target_from_model(&base, jan).unwrap()).unwrap();
        assert_eq!(skeleton.num_bones(), 72);
        assert!(skeleton.skeleton.iter().any(|b| b.name == "l_d5_j3"));
        let jan_ground = ground_mesh(&base, &skeleton, jan).unwrap();
        let kyle_ground = ground_mesh(&base, &skeleton, DEFAULT_TARGET_MODEL).unwrap();
        assert!((-26.0..-22.0).contains(&kyle_ground["floor_z"].as_f64().unwrap()));
        assert_eq!(jan_ground["model"], jan);
        assert!(!jan_ground["points"].as_array().unwrap().is_empty());
        assert_ne!(jan_ground["points"], kyle_ground["points"]);
        assert!(target_from_model(&base, "../outside.glm").is_err());
        assert!(target_from_model(&base, "models/players/missing/model.glm").is_err());
    }
}
