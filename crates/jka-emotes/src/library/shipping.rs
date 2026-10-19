//! Playback-only packing; authoring bundles remain the reproducible source.
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub fn run(
    source: &Path,
    input: &Path,
    output: &Path,
    profile: &str,
    jobs: usize,
) -> Result<(), String> {
    if !matches!(profile, "small" | "lossless" | "tiny") {
        return Err("profile must be small, lossless or tiny".into());
    }
    if output.exists() {
        return Err(format!("output already exists: {}", output.display()));
    }
    let mut files = fs::read_dir(input)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    files.retain(|p| {
        p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("djk_fnemote"))
    });
    files.sort();
    if files.is_empty() {
        return Err("no authoring bundles found".into());
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let files = Arc::new(files);
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, 8) {
            let tx = tx.clone();
            let files = files.clone();
            let next = &next;
            scope.spawn(move || loop {
                let n = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(n) else {
                    break;
                };
                let result = pack(
                    source,
                    path,
                    &output.join(path.file_name().unwrap()),
                    profile,
                );
                if tx.send((path.clone(), result)).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        let mut records = Vec::new();
        let mut errors = Vec::new();
        for (path, result) in rx {
            match result {
                Ok(record) => records.push(record),
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
            let done = records.len() + errors.len();
            if done % 25 == 0 || done == files.len() {
                println!("Packed {done}/{} ({} errors)", files.len(), errors.len());
            }
        }
        records.sort_by_key(|r| r["id"].as_str().unwrap_or("").to_owned());
        let before: u64 = records.iter().filter_map(|r| r["before"].as_u64()).sum();
        let after: u64 = records.iter().filter_map(|r| r["after"].as_u64()).sum();
        fs::write(output.join("shipping-report.json"), serde_json::to_vec_pretty(&json!({
            "profile":profile,"before_bytes":before,"after_bytes":after,"bundles":records,"errors":errors,
            "animation":if profile == "tiny" {"bounded fixed-point precision and Zstandard"} else {"lossless float prediction and Zstandard; redundant GLA frames omitted"},
            "audio":if profile == "tiny" {"copied unchanged from input (Vorbis or FLAC)"} else if profile == "lossless" {"lossless FLAC"} else {"mono Vorbis VBR target 48 kbps"}
        })).unwrap()).map_err(|e| e.to_string())?;
        println!("Shipping library: {before} -> {after} bytes");
        if errors.is_empty() {
            let links = input.join("catalog-links.json");
            if links.is_file() {
                fs::copy(links, output.join("catalog-links.json")).map_err(|e| e.to_string())?;
            }
            Ok(())
        } else {
            Err(format!(
                "{} bundles failed; see shipping-report.json",
                errors.len()
            ))
        }
    })
}

pub(super) fn pack(
    source: &Path,
    input: &Path,
    output: &Path,
    profile: &str,
) -> Result<Value, String> {
    let bundle = Bundle::open_index(input)?;
    if profile == "tiny" {
        if bundle.manifest["rig_only"] != true {
            return Err("tiny input must be a small/lossless shipping library".into());
        }
        return pack_playback(input, output, &bundle);
    }
    let recipe: Value =
        serde_json::from_slice(&bundle.read("metadata/recipe.json", MAX_MEMBER_BYTES)?)
            .map_err(|e| e.to_string())?;
    let mut root = bundle.manifest.clone();
    let id = root["emotes"][0]["id"]
        .as_str()
        .ok_or("missing ID")?
        .to_owned();
    let source = source.join(&id).canonicalize().map_err(|e| e.to_string())?;
    let gla = jka_assets::ghoul2::parse_gla(&bundle.read("animation.gla", MAX_MEMBER_BYTES)?)?;
    let rig = crate::retarget::encode(
        &gla,
        1,
        &vec![(glam::Quat::IDENTITY, glam::Vec3::ZERO); gla.skeleton.len()],
    )?;
    let pose = bundle.read("animation.pose", MAX_MEMBER_BYTES)?;
    let packed_pose = crate::PoseClip::pack_lossless(&pose)?;
    // Verify every float bit before shipping the new representation.
    let decoded = crate::PoseClip::parse(&packed_pose)?;
    if crate::PoseClip::encode(decoded.frames, decoded.bones, decoded.matrices())? != pose {
        return Err("lossless pose roundtrip failed".into());
    }
    let mut members = BTreeMap::from([
        ("animation.gla".to_owned(), rig),
        ("animation.pose".to_owned(), packed_pose),
    ]);
    let definitions = recipe["animation"]["variants"]
        .as_array()
        .ok_or("authoring bundle lacks montage recipes")?;
    let variants = root["emotes"][0]["variants"]
        .as_array_mut()
        .ok_or("bundle lacks variants")?;
    if variants.len() != definitions.len() {
        return Err("recipe/variant mismatch".into());
    }
    let mut warnings = Vec::new();
    for (row, definition) in variants.iter_mut().zip(definitions) {
        let mut cues = batch::resolve_montage_audio(
            &source,
            definition["montage"]
                .as_str()
                .unwrap_or(text(definition, "name")?),
            definition["segments"]
                .as_array()
                .ok_or("missing segments")?,
            definition["spans"].as_array().ok_or("missing spans")?,
            definition["duration_seconds"]
                .as_f64()
                .ok_or("missing duration")?,
            definition["loop_start_seconds"].as_f64(),
            &mut warnings,
        )?;
        let duration_ms =
            ((row["end_frame"].as_f64().unwrap() - row["first_frame"].as_f64().unwrap()) * 1000.0
                / 30.0)
                .round() as u64;
        let source_end = (definition["duration_seconds"].as_f64().unwrap() * 1000.0).round() as u64;
        for event in &mut cues {
            if event["at_ms"].as_u64() == Some(source_end) && event["repeat_period_ms"].is_null() {
                event["at_ms"] = json!(duration_ms);
            }
            if event["mix"].is_null() {
                continue;
            }
            let wav = audio_mix::render(&source, &event["mix"])?;
            event["silent_pcm"] = json!(audio_mix::is_silent(&wav)?);
            if let Some(start) = event["loop_start_seconds"].as_f64() {
                let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
                let sample = (start * rate as f64).round() as usize;
                if sample >= (wav.len() - 44) / 2 {
                    return Err("loop point exceeds PCM".into());
                }
                event["loop_start_sample"] = json!(sample);
                event.as_object_mut().unwrap().remove("loop_start_seconds");
            }
            let bytes = if profile == "small" {
                crate::compress_pcm16_vorbis(&wav)?
            } else {
                audio_mix::compress(&wav)?
            };
            crate::validate_audio(&bytes)?;
            let member = members
                .iter()
                .find(|(n, b)| n.starts_with("audio/") && **b == bytes)
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| {
                    let name = format!(
                        "audio/{:04}.{}",
                        members.len(),
                        if profile == "small" { "ogg" } else { "flac" }
                    );
                    members.insert(name.clone(), bytes);
                    name
                });
            event.as_object_mut().unwrap().remove("mix");
            event["wav"] = json!(member);
        }
        row["audio"] = json!(cues);
        refresh_audio_status(row);
        row.as_object_mut().unwrap().remove("conversion");
    }
    let primary = variants[0].clone();
    for key in ["audio", "audio_status", "audio_detail"] {
        root["emotes"][0][key] = primary[key].clone();
    }
    root.as_object_mut().unwrap().remove("conversion");
    root.as_object_mut().unwrap().remove("source_audio");
    root["rig_only"] = json!(true);
    root["shipping_profile"] = json!(profile);
    crate::write_shipping_bundle(output, &root, &members)?;
    Ok(
        json!({"id":id,"before":fs::metadata(input).map_err(|e|e.to_string())?.len(),
        "after":fs::metadata(output).map_err(|e|e.to_string())?.len(),"warnings":warnings}),
    )
}

fn pack_playback(input: &Path, output: &Path, bundle: &Bundle) -> Result<Value, String> {
    let mut root = bundle.manifest.clone();
    refresh_audio_status(&mut root["emotes"][0]);
    let mut members = BTreeMap::new();
    members.insert(
        "animation.gla".to_owned(),
        bundle.read("animation.gla", MAX_MEMBER_BYTES)?,
    );
    let pose = bundle.read("animation.pose", MAX_MEMBER_BYTES)?;
    let compact = crate::PoseClip::pack_compact(&pose)?;
    let before = crate::PoseClip::parse(&pose)?;
    let after = crate::PoseClip::parse(&compact)?;
    let mut max_linear = 0.0f32;
    let mut max_translation = 0.0f32;
    for (a, b) in before.matrices().iter().zip(after.matrices()) {
        for r in 0..3 {
            for c in 0..4 {
                let error = (a[r][c] - b[r][c]).abs();
                if c == 3 {
                    max_translation = max_translation.max(error);
                } else {
                    max_linear = max_linear.max(error);
                }
            }
        }
    }
    if max_linear > 0.0000051 || max_translation > 0.00051 {
        return Err("compact pose exceeds precision bound".into());
    }
    root["animation_precision"] = json!({"max_linear_error":max_linear,"max_translation_error":max_translation,
        "encoding":if compact.starts_with(b"DJKPOSE3") {"fixed-point delta"} else {"lossless fallback"}});
    members.insert("animation.pose".to_owned(), compact);
    for emote in crate::manifest::parse_manifest(&root)? {
        for event in emote
            .audio
            .iter()
            .chain(emote.variants.iter().flat_map(|v| &v.audio))
        {
            if let Some(name) = &event.wav {
                if !members.contains_key(name) {
                    members.insert(name.clone(), bundle.read(name, crate::MAX_WAV_BYTES)?);
                }
            }
        }
    }
    root["shipping_profile"] = json!("tiny");
    crate::write_shipping_bundle(output, &root, &members)?;
    Ok(
        json!({"id":root["emotes"][0]["id"],"before":fs::metadata(input).map_err(|e| e.to_string())?.len(),
        "after":fs::metadata(output).map_err(|e| e.to_string())?.len(),"precision":root["animation_precision"]}),
    )
}

fn refresh_audio_status(row: &mut Value) {
    let audio: Vec<_> = row["audio"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["wav"].is_string())
        .collect();
    let music: Vec<_> = audio.iter().filter(|e| e["lane"] == "music").collect();
    if !audio.is_empty() {
        let (status, detail) = if audio.iter().all(|e| e["silent_pcm"] == true) {
            ("silent_mix", "Resolved source tracks render to silent PCM; inspect source samples and graph gain/timing.")
        } else if !music.is_empty() && music.iter().all(|e| e["silent_pcm"] == true) {
            (
                "partial",
                "Music tracks render to silent PCM; only other sound effects are audible.",
            )
        } else if row["audio_status"] == "silent_mix"
            || (row["audio_status"] == "partial"
                && row["audio_detail"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("Music events render")))
        {
            if music.is_empty() {
                (
                    "foley_only",
                    "Sound effects resolved; no music event is scheduled.",
                )
            } else {
                (
                    "available",
                    "Music resolved and rendered from exported source metadata.",
                )
            }
        } else {
            ("", "")
        };
        if !status.is_empty() {
            row["audio_status"] = json!(status);
            row["audio_detail"] = json!(detail);
        }
    }
    if let Some(variants) = row["variants"].as_array_mut() {
        for variant in variants {
            refresh_audio_status(variant);
        }
    }
}
