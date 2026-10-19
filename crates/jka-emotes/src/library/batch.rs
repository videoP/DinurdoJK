//! Export one item per bundle, composing its montages and grouping variants.
use super::*;
use std::{
    collections::{BTreeSet, HashSet},
    io::Write,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Job {
    source: PathBuf,
    psa: PathBuf,
    clips: Vec<PathBuf>,
    id: String,
    label: String,
}

pub fn run(
    source: &Path,
    target_base: &Path,
    output: &Path,
    jobs: usize,
    filter: Option<&str>,
    target_model: &str,
    metadata: Option<&Path>,
) -> Result<(), String> {
    if !(1..=8).contains(&jobs) {
        return Err("jobs must be between 1 and 8".into());
    }
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    // Recipes resolve members beneath source_root. Keep supplemental roots
    // absolute too, so relative CLI arguments cannot be appended twice.
    let metadata_root = metadata
        .map(Path::canonicalize)
        .transpose()
        .map_err(|e| e.to_string())?;
    let metadata = metadata_root.as_deref();
    let target_base = target_base.canonicalize().map_err(|e| e.to_string())?;
    let target = jka_assets::ghoul2::parse_gla(&target_from_model(&target_base, target_model)?)?;
    let ground = ground_mesh(&target_base, &target, target_model)?;
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let output = output.canonicalize().map_err(|e| e.to_string())?;
    fs::create_dir_all(output.join("recipes")).map_err(|e| e.to_string())?;
    let mut dirs = fs::read_dir(&source)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    dirs.sort_by_key(|e| e.file_name());
    let mut work = Vec::new();
    let mut empty = Vec::new();
    let mut ids = HashSet::new();
    for dir in dirs {
        if !dir.file_type().map_err(|e| e.to_string())?.is_dir()
            || !dir.path().join("ExportReport.json").is_file()
        {
            continue;
        }
        let root = dir.path().canonicalize().map_err(|e| e.to_string())?;
        if !root.starts_with(&source) {
            return Err("emote source escapes source directory".into());
        }
        let report = load_json(&root.join("ExportReport.json"))?;
        let eid = text(&report, "Id")?;
        if filter
            .is_some_and(|ids| ids != "-" && !ids.split(',').any(|id| id.eq_ignore_ascii_case(eid)))
        {
            continue;
        }
        let label = report["Name"].as_str().unwrap_or(eid);
        // A primary montage can be absent while its authored alternative is
        // available. Discover clips in the supplemental item before classifying
        // the source definition as unavailable.
        let overlay = metadata.map(|m| m.join("Source").join(eid));
        let clip_root = overlay
            .as_ref()
            .filter(|p| p.join("ExtraMontages.json").is_file())
            .unwrap_or(&root);
        let mut clips = Vec::new();
        if clip_root.join("Animations").is_dir() {
            for clip in fs::read_dir(clip_root.join("Animations")).map_err(|e| e.to_string())? {
                let clip = clip.map_err(|e| e.to_string())?;
                if clip.file_type().map_err(|e| e.to_string())?.is_file()
                    && clip
                        .path()
                        .extension()
                        .is_some_and(|v| v.eq_ignore_ascii_case("psa"))
                {
                    clips.push(clip.path());
                }
            }
        }
        clips.sort();
        if clips.is_empty() {
            empty.push(json!({"id":eid,"name":label,"errors":report["Errors"]}));
        }
        if let Some(psa) = clips.first() {
            let id = safe_name(eid);
            if !ids.insert(id.clone()) {
                return Err(format!("batch filename collision: {id}"));
            }
            work.push(Job {
                source: root.clone(),
                psa: psa.clone(),
                clips: clips.clone(),
                id,
                label: label.to_owned(),
            });
        }
    }
    if work.is_empty() {
        return Err("no exported PSA animations found".into());
    }
    println!(
        "Batch: {} emote items, {} definitions without clips, {jobs} workers",
        work.len(),
        empty.len()
    );
    let mut log =
        fs::File::create(output.join("batch-results.jsonl")).map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    let next = AtomicUsize::new(0);
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            let send = send.clone();
            let work = &work;
            let next = &next;
            let target = &target;
            let ground = &ground;
            let target_base = &target_base;
            let output = &output;
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1,Ordering::Relaxed);
                    let Some(job) = work.get(index) else { break; };
                    let result = process(job,target_base,target,ground,output,target_model,metadata);
                    let row = match result {
                        Ok((status, warnings, cues)) => json!({"id":job.id,"psa":job.psa,"status":status,"warnings":warnings,"audio_events":cues,
                            "output":output.join(format!("{}.djk_fnemote",job.id))}),
                        Err(error) => json!({"id":job.id,"psa":job.psa,"status":"failed","error":error}),
                    };
                    if send.send(row).is_err() { break; }
                }
            });
        }
        drop(send);
        for row in receive {
            serde_json::to_writer(&mut log, &row).map_err(|e| e.to_string())?;
            writeln!(log).map_err(|e| e.to_string())?;
            log.flush().map_err(|e| e.to_string())?;
            results.push(row);
            if results.len() % 25 == 0 || results.len() == work.len() {
                let failed = results.iter().filter(|r| r["status"] == "failed").count();
                println!(
                    "Processed {}/{} ({failed} failures)",
                    results.len(),
                    work.len()
                );
            }
        }
        Ok::<_, String>(())
    })?;
    results.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let failed = results.iter().filter(|r| r["status"] == "failed").count();
    let existing = results.iter().filter(|r| r["status"] == "existing").count();
    let mut audio_issues = Vec::new();
    for row in results.iter().filter(|r| r["status"] != "failed") {
        let bundle =
            Bundle::open_index(Path::new(row["output"].as_str().ok_or("missing output")?))?;
        let parent = &bundle.manifest["emotes"][0];
        let variants = parent["variants"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![parent.clone()]);
        for v in variants {
            if v["audio_status"] != "available" {
                audio_issues.push(json!({"id":parent["id"],"title":parent["name"],"variation":v["variant_name"],"status":v["audio_status"],"detail":v["audio_detail"],"source":source.join(parent["id"].as_str().unwrap_or(""))}));
            }
        }
    }
    fs::write(
        output.join("audio-issues.json"),
        serde_json::to_vec_pretty(&audio_issues).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let report = json!({"source":source,"target_base":target_base,"output":output,
        "target_model":target_model,"summary":{"emote_items":work.len(),"compiled":work.len()-failed-existing,"existing":existing,"failed":failed,"definitions_without_psa":empty.len(),"variations_with_audio_issues":audio_issues.len()},
        "policy":"One item per bundle; montage trims/rates/repeats/section loops and variants composed on the native 72-bone humanoid rig. Compressed source audio and JSON retained; supported SoundCue and MetaSound template tracks stored as deduplicated lossless FLAC, with independent music loop timing. Props and VFX omitted.",
        "results":results,"definitions_without_psa":empty});
    fs::write(
        output.join("batch-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        output.join("catalog-links.json"),
        serde_json::to_vec_pretty(&crate::library::catalog::index_source(&source)?).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "Finished: {} usable bundles, {failed} failed clips, {existing} reused. Report: {}",
        work.len() - failed,
        output.join("batch-report.json").display()
    );
    if failed > 0 {
        return Err(format!(
            "{failed} emote bundles failed; see batch-report.json"
        ));
    }
    Ok(())
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
pub(super) fn load_json(path: &Path) -> Result<Value, String> {
    let bytes = read(path, MAX_MEMBER_BYTES)?;
    serde_json::from_slice(bytes.strip_prefix(&[239, 187, 191]).unwrap_or(&bytes))
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn process(
    job: &Job,
    target_base: &Path,
    target: &jka_assets::ghoul2::GlaAnimation,
    ground: &Value,
    output: &Path,
    target_model: &str,
    metadata: Option<&Path>,
) -> Result<(&'static str, Vec<String>, usize), String> {
    let mut warnings = Vec::new();
    let overlay = metadata.map(|m| m.join("Source").join(&job.id));
    let root = overlay
        .as_ref()
        .filter(|p| p.join("ExtraMontages.json").is_file())
        .unwrap_or(&job.source);
    let mut clips = job.clips.clone();
    if root != &job.source {
        clips = fs::read_dir(root.join("Animations"))
            .map_err(|e| e.to_string())?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        clips.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("psa")));
        clips.sort();
    }
    let supplement = metadata
        .map(|m| load_json(&m.join(format!("{}.json", job.id))))
        .transpose()?;
    let variants = if supplement.is_some() {
        discovery::definitions_with_metadata(root, &clips, &mut warnings, supplement.as_ref())?
    } else {
        discovery::definitions(root, &clips, &mut warnings)?
    };
    let cues = variants
        .iter()
        .map(|v| v["audio"].as_array().map_or(0, Vec::len))
        .sum();
    let recipe = json!({"id":job.id,"name":job.label,"source_root":root,
        "animation":{"variants":variants,"target_base":target_base,"target_model":target_model,"retarget":{"ground_mesh":ground}},
        "audio":[],"include_all_audio":false,"include_compressed_sources":true,"batch":{"policy_version":9,"warnings":warnings}});
    let destination = output.join(format!("{}.djk_fnemote", job.id));
    if destination.exists() {
        let bundle = Bundle::open_index(&destination)?;
        let old: Value = serde_json::from_slice(
            &bundle.read("metadata/recipe.json", crate::MAX_MANIFEST_BYTES)?,
        )
        .map_err(|e| e.to_string())?;
        if !matching_recipe(&old, &recipe)? {
            return Err(
                "existing bundle has a different recipe; move it aside before recompiling".into(),
            );
        }
        return Ok(("existing", warnings, cues));
    }
    fs::write(
        output.join("recipes").join(format!("{}.json", job.id)),
        serde_json::to_vec_pretty(&recipe).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    compile_recipe(&recipe, root, &destination, Some(target))?;
    Ok(("compiled", warnings, cues))
}

// serde_json's default float reader can round the last bit differently from
// an in-memory f32->f64 value. Compare both through the persisted JSON format.
fn matching_recipe(stored: &Value, fresh: &Value) -> Result<bool, String> {
    let persisted: Value =
        serde_json::from_slice(&serde_json::to_vec(fresh).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    Ok(*stored == persisted)
}

// Bake supported exported sound graphs into one track per lane/audience.
pub(super) fn resolve_audio(
    root: &Path,
    stem: &str,
    duration: f64,
    warnings: &mut Vec<String>,
) -> Result<Vec<Value>, String> {
    let file = root.join("Audio/AudioReport.json");
    if !file.is_file() {
        return Ok(Vec::new());
    }
    let report = load_json(&file)?;
    resolve_audio_report(root, stem, duration, warnings, report)
}
fn resolve_audio_report(
    root: &Path,
    stem: &str,
    duration: f64,
    warnings: &mut Vec<String>,
    mut report: Value,
) -> Result<Vec<Value>, String> {
    report["_source_root"] = json!(root);
    report["_duration"] = json!(duration);
    let mut montages = BTreeMap::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry
            .path()
            .extension()
            .is_some_and(|v| v.eq_ignore_ascii_case("json"))
        {
            let value = load_json(&entry.path())?;
            if value["Type"] == "AnimMontage" {
                montages.insert(value["Name"].as_str().unwrap_or("").to_owned(), value);
            }
        }
    }
    let mut tracks: BTreeMap<(&str, &str), Vec<Value>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for event in report["Events"].as_array().into_iter().flatten() {
        let owner = event["Animation"]
            .as_str()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("");
        let mut at = event["StartSeconds"].as_f64().unwrap_or(0.0);
        let mut end = event["EndSeconds"].as_f64();
        if !owner.eq_ignore_ascii_case(stem) {
            let Some(montage) = montages.get(owner) else {
                continue;
            };
            let segments: Vec<_> = montage["Properties"]["SlotAnimTracks"]
                .as_array()
                .into_iter()
                .flatten()
                // The exporter already resolves corrective PSA poses against
                // FullBody. They therefore share the montage's sound timeline.
                .filter(|t| t["SlotName"] == "FullBody" || t["SlotName"] == "AdditiveCorrective")
                .flat_map(|t| {
                    t["AnimTrack"]["AnimSegments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                })
                .filter(|s| object_name(&s["AnimReference"]).eq_ignore_ascii_case(stem))
                .collect();
            if segments.len() != 1 {
                continue;
            }
            let segment = segments[0];
            if (segment["AnimPlayRate"].as_f64().unwrap_or(1.0) - 1.0).abs() > 0.0001
                || segment["LoopingCount"].as_u64().unwrap_or(1) > 1
            {
                warnings.push(format!(
                    "Audio timing for {owner} needs montage rate/repeat reconstruction"
                ));
                continue;
            }
            let offset = segment["AnimStartTime"].as_f64().unwrap_or(0.0)
                - segment["StartPos"].as_f64().unwrap_or(0.0);
            at += offset;
            end = end.map(|t| t + offset);
            let start = segment["AnimStartTime"].as_f64().unwrap_or(0.0);
            let stop = segment["AnimEndTime"].as_f64().unwrap_or(f64::INFINITY);
            if at < start - 0.001 || at >= stop {
                continue;
            }
        }
        // Float export noise can put an authored time-zero notify a few
        // microseconds before zero; it still owns the opening audio.
        if at >= -0.0001 && at < 0.0 {
            at = 0.0;
        }
        if !at.is_finite() || at < 0.0 {
            continue;
        }
        let props = &event["Notify"]["Properties"];
        for (key, audience, lane) in [
            ("EmoteSound1P", "1p", "music"),
            ("EmoteSound3P", "3p", "music"),
            ("Sound", "both", "foley"),
        ] {
            if props[key].is_null() {
                continue;
            }
            let Some(voices) = resolve_wave(&props[key], &report, warnings, &mut HashSet::new(), 0)
            else {
                warnings.push(format!("Unresolved {key} graph on {owner} at {at}s"));
                continue;
            };
            let ms = (at * 1000.0).round() as u64;
            if !seen.insert((
                ms,
                audience.to_string(),
                lane.to_string(),
                serde_json::to_string(&voices).unwrap(),
            )) {
                continue;
            }
            for mut voice in voices {
                let path = voice["wav"].as_str().ok_or("graph voice missing WAV")?;
                let bytes = read(
                    &relative(&root.canonicalize().map_err(|e| e.to_string())?, path)?,
                    MAX_MEMBER_BYTES,
                )?;
                if let Err(error) = crate::validate_wav(&bytes) {
                    warnings.push(format!("Unscheduled {path}: {error}"));
                    continue;
                }
                voice["at_seconds"] = json!(at + voice["at_seconds"].as_f64().unwrap_or(0.0));
                if voice["at_seconds"].as_f64().unwrap_or(duration) >= duration {
                    continue;
                }
                if props["bStopAudioOnNotifyEnd"].as_bool().unwrap_or(true) {
                    if let Some(end) = end.filter(|t| t.is_finite() && *t > at) {
                        voice["stop_seconds"] = json!(end.min(duration));
                    }
                }
                tracks.entry((audience, lane)).or_default().push(voice);
            }
        }
    }
    let output=tracks.into_iter().map(|((audience,lane),voices)|json!({"at_ms":0,"lane":lane,"audience":audience,"mix":{"duration_seconds":duration,"voices":voices}})).collect();
    warnings.sort();
    warnings.dedup();
    Ok(output)
}
pub(super) fn object_name(value: &Value) -> String {
    let name = value["ObjectName"].as_str().unwrap_or("");
    name.split('\'').nth(1).unwrap_or(name).to_owned()
}
fn resolve_wave(
    reference: &Value,
    report: &Value,
    warnings: &mut Vec<String>,
    visited: &mut HashSet<String>,
    depth: usize,
) -> Option<Vec<Value>> {
    if depth > 32 {
        return None;
    }
    let path = reference["ObjectPath"]
        .as_str()
        .or(reference["AssetPathName"].as_str())
        .or(reference.as_str())?;
    let package = path.rsplit_once('.').map(|p| p.0).unwrap_or(path);
    if let Some(clip) = report["Clips"].as_array()?.iter().find(|c| {
        c["Asset"]
            .as_str()
            .is_some_and(|a| a.rsplit_once('.').map(|p| p.0) == Some(package))
    }) {
        return clip["Playable"].as_str().map(|wav| {
            vec![json!({"wav":format!("Audio/{wav}"),"at_seconds":0.0,"gain":1.0,"pitch":1.0})]
        });
    }
    let name = object_name(reference);
    let graph = report["Graph"].as_object()?;
    let key = if graph.contains_key(path) {
        path.to_owned()
    } else {
        format!("{package}.{name}")
    };
    if !visited.insert(key.clone()) {
        return None;
    }
    let node = graph.get(&key)?;
    let props = &node["Properties"];
    match node["Type"].as_str()? {
        "MetaSoundSource" => resolve_metasound(props, report, warnings, visited, depth),
        "SoundCue" | "EmoteFoley" | "EmoteMusic" | "EmoteMusic3P" => {
            let mut voices =
                resolve_wave(&props["FirstNode"], report, warnings, visited, depth + 1)?;
            for voice in &mut voices {
                voice["gain"] = json!(
                    voice["gain"].as_f64()? * props["VolumeMultiplier"].as_f64().unwrap_or(1.0)
                );
                voice["pitch"] = json!(
                    voice["pitch"].as_f64()? * props["PitchMultiplier"].as_f64().unwrap_or(1.0)
                );
            }
            Some(voices)
        }
        "SoundNodeWavePlayer" => {
            let mut voices = resolve_wave(
                &props["SoundWaveAssetPtr"],
                report,
                warnings,
                visited,
                depth + 1,
            )
            .or_else(|| {
                resolve_wave(
                    &node["SoundWave"],
                    report,
                    warnings,
                    &mut visited.clone(),
                    depth + 1,
                )
            })?;
            for voice in &mut voices {
                voice["looping"] = json!(props["bLooping"].as_bool().unwrap_or(false));
            }
            Some(voices)
        }
        "SoundNodeMixer" => {
            let mut voices = Vec::new();
            for (i, child) in props["ChildNodes"].as_array()?.iter().enumerate() {
                if child.is_null() {
                    continue;
                }
                let mut branch =
                    resolve_wave(child, report, warnings, &mut visited.clone(), depth + 1)?;
                for voice in &mut branch {
                    voice["gain"] = json!(
                        voice["gain"].as_f64()? * props["InputVolume"][i].as_f64().unwrap_or(1.0)
                    );
                }
                voices.extend(branch);
            }
            Some(voices)
        }
        "SoundNodeEnveloper" => {
            // Envelope loops/weighted tangents need a different bake; preserve
            // them as unresolved instead of silently changing their timing.
            for curve in ["VolumeCurve", "PitchCurve"] {
                if props[curve]["EditorCurveData"]["Keys"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|k| {
                        k["TangentWeightMode"]
                            .as_str()
                            .is_some_and(|m| m != "RCTWM_WeightedNone")
                    })
                {
                    return None;
                }
            }
            let mut voices = Vec::new();
            for child in props["ChildNodes"].as_array()? {
                let mut branch =
                    resolve_wave(child, report, warnings, &mut visited.clone(), depth + 1)?;
                for voice in &mut branch {
                    let mut envelopes = voice["envelopes"].as_array().cloned().unwrap_or_default();
                    let looped = props["bLoop"].as_bool().unwrap_or(false)
                        || props["bLoopIndefinitely"].as_bool().unwrap_or(false);
                    let cycle = if looped {
                        json!({"start":props["LoopStart"].as_f64().unwrap_or(0.0),"end":props["LoopEnd"],"count":if props["bLoopIndefinitely"].as_bool().unwrap_or(false) {Value::Null}else{props["LoopCount"].clone()}})
                    } else {
                        Value::Null
                    };
                    envelopes.push(json!({"volume":props["VolumeCurve"]["EditorCurveData"]["Keys"],"pitch":props["PitchCurve"]["EditorCurveData"]["Keys"],"offset":voice["at_seconds"],"cycle":cycle}));
                    voice["envelopes"] = json!(envelopes);
                }
                voices.extend(branch);
            }
            Some(voices)
        }
        "SoundNodeLooping" => {
            let children = props["ChildNodes"].as_array()?;
            if children.len() != 1 {
                return None;
            }
            let mut voices = resolve_wave(&children[0], report, warnings, visited, depth + 1)?;
            if voices.len() != 1
                || voices[0]["at_seconds"].as_f64().unwrap_or(0.0) != 0.0
                || !voices[0]["envelopes"].is_null()
            {
                if voices
                    .iter()
                    .any(|v| v["looping"].as_bool().unwrap_or(false))
                {
                    return Some(voices);
                }
                let root = Path::new(report["_source_root"].as_str()?);
                let mut period = 0.0f64;
                for voice in &voices {
                    let bytes = read(
                        &relative(root, voice["wav"].as_str()?).ok()?,
                        MAX_MEMBER_BYTES,
                    )
                    .ok()?;
                    period = period.max(
                        voice["at_seconds"].as_f64()?
                            + audio_mix::duration(&bytes).ok()? / voice["pitch"].as_f64()?,
                    );
                }
                if period <= 0.0 {
                    return None;
                }
                let count = if props["bLoopIndefinitely"].as_bool().unwrap_or(true) {
                    (report["_duration"].as_f64().unwrap_or(period) / period).ceil() as usize
                } else {
                    props["LoopCount"].as_u64().unwrap_or(1) as usize
                };
                if count.saturating_mul(voices.len()) > 4096 {
                    return None;
                }
                let mut repeated = Vec::new();
                for repeat in 0..count {
                    for voice in &voices {
                        let mut voice = voice.clone();
                        voice["at_seconds"] =
                            json!(voice["at_seconds"].as_f64()? + repeat as f64 * period);
                        repeated.push(voice);
                    }
                }
                return Some(repeated);
            }
            voices[0]["looping"] = json!(true);
            if !props["bLoopIndefinitely"].as_bool().unwrap_or(true) {
                voices[0]["loop_count"] = json!(props["LoopCount"].as_u64().unwrap_or(1));
            }
            Some(voices)
        }
        "SoundNodeDelay" | "SoundNodeModulator" | "SoundNodeModulatorContinuous" => {
            let midpoint = |min: &str, max: &str, default: f64| {
                (props[min].as_f64().unwrap_or(default) + props[max].as_f64().unwrap_or(default))
                    * 0.5
            };
            let mut voices = Vec::new();
            for child in props["ChildNodes"].as_array()? {
                let mut branch =
                    resolve_wave(child, report, warnings, &mut visited.clone(), depth + 1)?;
                for voice in &mut branch {
                    if node["Type"] == "SoundNodeDelay" {
                        voice["at_seconds"] = json!(
                            voice["at_seconds"].as_f64()? + midpoint("DelayMin", "DelayMax", 0.0)
                        );
                    } else if node["Type"] == "SoundNodeModulatorContinuous" {
                        let parameter = |key: &str| {
                            let p = &props[key];
                            let lo = p["MinInput"].as_f64().unwrap_or(0.0);
                            let hi = p["MaxInput"].as_f64().unwrap_or(1.0);
                            let value = p["Default"].as_f64().unwrap_or(0.0);
                            let t = if hi > lo {
                                ((value - lo) / (hi - lo)).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let min = p["MinOutput"].as_f64().unwrap_or(1.0);
                            min + (p["MaxOutput"].as_f64().unwrap_or(1.0) - min) * t
                        };
                        voice["gain"] =
                            json!(voice["gain"].as_f64()? * parameter("VolumeModulationParams"));
                        voice["pitch"] =
                            json!(voice["pitch"].as_f64()? * parameter("PitchModulationParams"));
                        warnings.push(
                            "Continuous sound parameters rendered at exported default (idle)"
                                .into(),
                        );
                    } else {
                        voice["gain"] = json!(
                            voice["gain"].as_f64()? * midpoint("VolumeMin", "VolumeMax", 1.0)
                        );
                        voice["pitch"] =
                            json!(voice["pitch"].as_f64()? * midpoint("PitchMin", "PitchMax", 1.0));
                    }
                }
                voices.extend(branch);
            }
            Some(voices)
        }
        "FortSoundNodeLicensedContentSwitcher" => {
            let mut fallback = None;
            for child in props["ChildNodes"]
                .as_array()?
                .iter()
                .filter(|c| !c.is_null())
            {
                let mut branch_warnings = Vec::new();
                if let Some(voices) = resolve_wave(
                    child,
                    report,
                    &mut branch_warnings,
                    &mut visited.clone(),
                    depth + 1,
                ) {
                    let audible = voices.iter().any(|v| {
                        if v["gain"].as_f64().unwrap_or(1.0).abs() < 0.00001 {
                            return false;
                        }
                        let Some(wav) = v["wav"].as_str() else {
                            return false;
                        };
                        if let Some(root) = report["_source_root"].as_str() {
                            relative(Path::new(root), wav)
                                .and_then(|p| read(&p, crate::MAX_WAV_BYTES))
                                .ok()
                                .and_then(|b| audio_mix::is_silent(&b).ok())
                                .is_none_or(|silent| !silent)
                        } else {
                            !wav.to_ascii_lowercase().contains("5_seconds_silence")
                        }
                    });
                    if audible {
                        warnings.extend(branch_warnings);
                        warnings.push("Licensed-content switcher: selected decoded audible branch; muted alternatives retained".into());
                        return Some(voices);
                    }
                    if fallback.is_none() {
                        fallback = Some((voices, branch_warnings));
                    }
                }
            }
            fallback.map(|(voices, w)| {
                warnings.extend(w);
                voices
            })
        }
        "SoundNodeRandom"
        | "SoundNodeQualityLevel"
        | "SoundNodeAttenuation"
        | "SoundNodeSoundClass"
        | "SoundNodeDoppler" => {
            let children = props["ChildNodes"].as_array()?;
            if children.len() > 1 {
                warnings.push(format!(
                    "{}: selected first exported branch; alternatives retained",
                    node["Type"].as_str()?
                ));
            }
            children
                .iter()
                .filter(|child| !child.is_null())
                .find_map(|child| {
                    resolve_wave(child, report, warnings, &mut visited.clone(), depth + 1)
                })
        }
        "SoundNodeParamCrossFade" => {
            let mut voices = Vec::new();
            for (i, child) in props["ChildNodes"].as_array()?.iter().enumerate() {
                let fade = &props["CrossFadeInput"][i];
                let x = 0.0f64;
                let start = fade["FadeInDistanceStart"].as_f64().unwrap_or(0.0);
                let end = fade["FadeInDistanceEnd"].as_f64().unwrap_or(start);
                let out_start = fade["FadeOutDistanceStart"]
                    .as_f64()
                    .unwrap_or(f64::INFINITY);
                let out_end = fade["FadeOutDistanceEnd"].as_f64().unwrap_or(out_start);
                let gain = if end > start {
                    ((x - start) / (end - start)).clamp(0.0, 1.0)
                } else {
                    if x >= start {
                        1.0
                    } else {
                        0.0
                    }
                } * if out_end > out_start {
                    ((out_end - x) / (out_end - out_start)).clamp(0.0, 1.0)
                } else {
                    if x <= out_start {
                        1.0
                    } else {
                        0.0
                    }
                } * fade["Volume"].as_f64().unwrap_or(1.0);
                if gain <= 0.0 {
                    continue;
                }
                let mut branch =
                    resolve_wave(child, report, warnings, &mut visited.clone(), depth + 1)?;
                for voice in &mut branch {
                    voice["gain"] = json!(voice["gain"].as_f64()? * gain);
                }
                voices.extend(branch);
            }
            warnings.push("Sound crossfade rendered at idle movement speed".into());
            Some(voices)
        }
        "SoundNodeConcatenator" => {
            let mut voices = Vec::new();
            let mut at = 0.0;
            for (i, child) in props["ChildNodes"].as_array()?.iter().enumerate() {
                let mut branch =
                    resolve_wave(child, report, warnings, &mut visited.clone(), depth + 1)?;
                let mut duration = 0.0f64;
                for voice in &mut branch {
                    if voice["looping"].as_bool().unwrap_or(false) {
                        return None;
                    }
                    let root = Path::new(report["_source_root"].as_str()?);
                    let bytes = read(
                        &relative(root, voice["wav"].as_str()?).ok()?,
                        MAX_MEMBER_BYTES,
                    )
                    .ok()?;
                    let start = voice["at_seconds"].as_f64()?;
                    duration = duration
                        .max(start + audio_mix::duration(&bytes).ok()? / voice["pitch"].as_f64()?);
                    voice["at_seconds"] = json!(start + at);
                    voice["gain"] = json!(
                        voice["gain"].as_f64()? * props["InputVolume"][i].as_f64().unwrap_or(1.0)
                    );
                }
                voices.extend(branch);
                at += duration;
            }
            Some(voices)
        }
        _ => None,
    }
}

pub(super) fn resolve_montage_audio(
    root: &Path,
    name: &str,
    segments: &[Value],
    spans: &[Value],
    duration: f64,
    loop_start: Option<f64>,
    warnings: &mut Vec<String>,
) -> Result<Vec<Value>, String> {
    let path = root.join("Audio/AudioReport.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut report = load_json(&path)?;
    // A gender/corrective montage can share the exact authored FullBody
    // timeline while its export omits music notifies. Use that proven timeline
    // relationship, never a fuzzy display-title match, to recover its music.
    let mut montage_values = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            let v = load_json(&p)?;
            if v["Type"] == "AnimMontage" {
                montage_values.push(v);
            }
        }
    }
    let signature = |m: &Value| -> Option<String> {
        let tracks = m["Properties"]["SlotAnimTracks"].as_array()?;
        let full = tracks
            .iter()
            .find(|t| t["SlotName"] == "FullBody" || t["SlotName"] == "FullBodyNoAdditive")?;
        let rows: Vec<_> = full["AnimTrack"]["AnimSegments"]
            .as_array()?
            .iter()
            .map(|s| {
                json!([
                    object_name(&s["AnimReference"])
                        .to_ascii_lowercase()
                        .replace("_cmf", "_cmm"),
                    s["StartPos"],
                    s["AnimStartTime"],
                    s["AnimEndTime"],
                    s["AnimPlayRate"],
                    s["LoopingCount"]
                ])
            })
            .collect();
        Some(
            serde_json::to_string(&json!([
                rows,
                m["Properties"]["CompositeSections"].as_array().map(|a| a
                    .iter()
                    .map(|s| json!([s["SectionName"], s["NextSectionName"], s["LinkValue"]]))
                    .collect::<Vec<_>>())
            ]))
            .unwrap(),
        )
    };
    let owner = |e: &Value| {
        e["Animation"]
            .as_str()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_owned()
    };
    let music = |e: &Value| {
        !e["Notify"]["Properties"]["EmoteSound1P"].is_null()
            || !e["Notify"]["Properties"]["EmoteSound3P"].is_null()
    };
    let original = report["Events"].as_array().cloned().unwrap_or_default();
    if !original
        .iter()
        .any(|e| owner(e).eq_ignore_ascii_case(name) && music(e))
    {
        if let Some(target) = montage_values
            .iter()
            .find(|m| m["Name"] == name)
            .and_then(signature)
        {
            if let Some(donor) = montage_values.iter().find(|m| {
                m["Name"] != name
                    && signature(m).as_ref() == Some(&target)
                    && original.iter().any(|e| {
                        e["Animation"].as_str().unwrap_or("").rsplit('.').next()
                            == m["Name"].as_str()
                            && music(e)
                    })
            }) {
                let donor_name = donor["Name"].as_str().unwrap();
                let mut supplemented = original.clone();
                for event in original
                    .iter()
                    .filter(|e| owner(e) == donor_name && music(e))
                {
                    let mut e = event.clone();
                    e["Animation"] = json!(name);
                    e["Notify"]["Properties"]
                        .as_object_mut()
                        .unwrap()
                        .remove("Sound");
                    supplemented.push(e);
                }
                report["Events"] = json!(supplemented);
                warnings.push(format!("Music notifies for {name} recovered from equivalent FullBody montage {donor_name}"));
            }
        }
    }
    let mut events = Vec::new();
    for event in report["Events"].as_array().into_iter().flatten() {
        let owner = event["Animation"]
            .as_str()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("");
        if owner.eq_ignore_ascii_case(name) {
            events.push(event.clone());
            continue;
        }
        for segment in segments {
            let stem = Path::new(segment["psa"].as_str().unwrap_or(""))
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            if !owner.eq_ignore_ascii_case(&stem) {
                continue;
            }
            let from = segment["from"].as_f64().unwrap_or(0.0);
            let to = segment["to"].as_f64().unwrap_or(0.0);
            let rate = segment["rate"].as_f64().unwrap_or(1.0);
            let t = event["StartSeconds"].as_f64().unwrap_or(0.0);
            if t < from || t >= to || rate == 0.0 {
                continue;
            }
            for repeat in 0..segment["repeats"].as_u64().unwrap_or(1).min(4096) {
                let offset = segment["start"].as_f64().unwrap_or(0.0)
                    + repeat as f64 * (to - from) / rate.abs();
                let mut e = event.clone();
                e["Animation"] = json!(name);
                e["StartSeconds"] = json!(
                    offset
                        + if rate > 0.0 {
                            (t - from) / rate
                        } else {
                            (to - t) / -rate
                        }
                );
                if let Some(end) = event["EndSeconds"].as_f64() {
                    e["EndSeconds"] = json!(offset + (end.min(to) - from) / rate.abs());
                }
                events.push(e);
            }
        }
    }
    report["Events"] = json!(events);
    let montage_duration = spans
        .iter()
        .filter_map(|s| s["to"].as_f64())
        .fold(0.0, f64::max);
    let tracks = resolve_audio_report(root, name, montage_duration, warnings, report)?;
    let mut output = Vec::new();
    for track in tracks {
        if loop_start.is_some() && track["lane"] == "music" {
            let mut music = track["mix"]["voices"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if music.iter().any(|v| v["looping"] == true) {
                // An audio graph owns its loop, independent of montage section
                // jumps. Truncating music to a 1.6-second Floss pose destroyed
                // the song and restarted the first phrase on every pose loop.
                let mut music_end = 0.0f64;
                let mut music_loop = f64::INFINITY;
                for voice in &mut music {
                    let start = voice["at_seconds"].as_f64().unwrap_or(0.0);
                    let bytes = read(&relative(root, text(voice, "wav")?)?, MAX_MEMBER_BYTES)?;
                    let length =
                        audio_mix::duration(&bytes)? / voice["pitch"].as_f64().unwrap_or(1.0);
                    music_end = music_end.max(start + length);
                    if voice["looping"] == true {
                        music_loop = music_loop.min(start);
                    }
                    voice.as_object_mut().unwrap().remove("stop_seconds");
                }
                if music_end > music_loop && music_end <= 600.0 {
                    // A one-shot startup envelope must finish once, rather than
                    // being baked into every repeated cycle. Render a steady
                    // second cycle only for those graphs; retain its full period.
                    let startup_envelope = music.iter().filter(|v| v["looping"] == true).any(|v| {
                        v["envelopes"].as_array().into_iter().flatten().any(|e| {
                            e["cycle"].is_null()
                                && ["volume", "pitch"].iter().any(|key| {
                                    e[key].as_array().is_some_and(|keys| {
                                        keys.windows(2).any(|p| p[0]["Value"] != p[1]["Value"])
                                    })
                                })
                        })
                    });
                    let (end, start) = if startup_envelope {
                        (music_end + music_end - music_loop, music_end)
                    } else {
                        (music_end, music_loop)
                    };
                    output.push(
                        json!({"at_ms":0,"lane":"music","audience":track["audience"],
                        "loop_start_seconds":start,
                        "mix":{"duration_seconds":end,"voices":music}}),
                    );
                    continue;
                }
            }
        }
        let mut voices = Vec::new();
        for span in spans {
            let from = span["from"].as_f64().unwrap_or(0.0);
            let to = span["to"].as_f64().unwrap_or(0.0);
            let at = span["at"].as_f64().unwrap_or(0.0);
            for voice in track["mix"]["voices"].as_array().into_iter().flatten() {
                let start = voice["at_seconds"].as_f64().unwrap_or(0.0);
                let stop = voice["stop_seconds"].as_f64().unwrap_or(montage_duration);
                if start >= to || stop <= from {
                    continue;
                }
                let mut v = voice.clone();
                v["at_seconds"] = json!(at + start.max(from) - from);
                v["source_offset_seconds"] = json!((from - start).max(0.0));
                v["stop_seconds"] = json!(at + stop.min(to) - from);
                voices.push(v);
            }
        }
        let mut intro = track.clone();
        intro["mix"] = json!({"duration_seconds":duration,"voices":voices});
        output.push(intro);
        if let Some(loop_start) = loop_start {
            let mut repeat = Vec::new();
            for voice in voices {
                let start = voice["at_seconds"].as_f64().unwrap_or(0.0);
                let stop = voice["stop_seconds"].as_f64().unwrap_or(duration);
                if stop <= loop_start {
                    continue;
                }
                let mut v = voice;
                v["at_seconds"] = json!((start - loop_start).max(0.0));
                v["stop_seconds"] = json!(stop - loop_start);
                v["source_offset_seconds"] = json!(
                    v["source_offset_seconds"].as_f64().unwrap_or(0.0)
                        + (loop_start - start).max(0.0)
                );
                repeat.push(v);
            }
            if !repeat.is_empty() {
                output.push(json!({"at_ms":(duration*1000.0).round() as u64,"lane":track["lane"],"audience":track["audience"],"repeat_on_loop":true,"mix":{"duration_seconds":duration-loop_start,"voices":repeat}}));
            }
        }
    }
    Ok(output)
}

fn resolve_metasound(
    props: &Value,
    report: &Value,
    warnings: &mut Vec<String>,
    visited: &mut HashSet<String>,
    depth: usize,
) -> Option<Vec<Value>> {
    let graph = &props["RootMetasoundDocument"]["RootGraph"];
    let inputs = graph["Interface"]["Inputs"].as_array()?;
    let runtime_music_template = graph["PagedGraphs"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|page| page["Nodes"].as_array().into_iter().flatten())
        .any(|node| {
            matches!(
                node["Name"].as_str(),
                Some("MSS_Emote_Music1P" | "MSS_Emote_Music3P")
            )
        });
    let literal = |name: &str| -> Value {
        inputs
            .iter()
            .find(|i| i["Name"] == name)
            .map(|i| i["Defaults"][0]["Literal"].clone())
            .unwrap_or(Value::Null)
    };
    let number = |name: &str, default: f64| literal(name)["AsFloat"][0].as_f64().unwrap_or(default);
    let boolean =
        |name: &str, default: bool| literal(name)["AsBoolean"][0].as_bool().unwrap_or(default);
    let pair = if inputs.iter().any(|i| i["Name"] == "Music.Sounds") {
        ("Music.Sounds", "Music.Loop.Sounds")
    } else if inputs.iter().any(|i| i["Name"] == "Foley.Sounds") {
        ("Foley.Sounds", "Foley.Loop.Sounds")
    } else if inputs.iter().any(|i| i["Name"] == "Intro.Sounds") {
        ("Intro.Sounds", "Loop.Sounds")
    } else if inputs.iter().any(|i| i["Name"] == "Traversal.Sounds") {
        warnings.push("Traversal MetaSound rendered at idle movement speed".into());
        ("Traversal.Sounds", "Traversal.Loop.Sounds")
    } else {
        return None;
    };
    let mut voices = Vec::new();
    let mut intro_duration = 0.0;
    for (key, is_loop) in [(pair.0, false), (pair.1, true)] {
        let refs = literal(key);
        let Some(refs) = refs["AsUObject"].as_array() else {
            continue;
        };
        if refs.len() > 1 {
            warnings.push(format!(
                "MetaSound {key}: selected first exported random alternative"
            ));
        }
        let Some(reference) = refs.iter().find(|r| !r.is_null()) else {
            continue;
        };
        let mut branch =
            resolve_wave(reference, report, warnings, &mut visited.clone(), depth + 1)?;
        for voice in &mut branch {
            let gain = props["Volume"].as_f64().unwrap_or(1.0)
                * number(
                    if is_loop {
                        "LoopVolumeMultiplier"
                    } else {
                        "IntroVolumeMultiplier"
                    },
                    1.0,
                );
            voice["gain"] = json!(voice["gain"].as_f64().unwrap_or(1.0) * gain);
            voice["pitch"] = json!(number("PitchTemplateMultiplier", 1.0));
            let delay = if is_loop {
                number("Loop.DelayTime", 0.0)
                    + if boolean("DelayLoopByStart", false) {
                        intro_duration
                    } else {
                        0.0
                    }
            } else {
                // Fortnite supplies this synchronization parameter at playback.
                // The template's preview default (often 4.114s) is not a local
                // emote delay. Authored notify offsets remain unchanged.
                if pair.0 == "Music.Sounds" && runtime_music_template {
                    0.0
                } else {
                    number("IntroDelay", 0.0)
                }
            };
            voice["at_seconds"] = json!(delay);
            voice["looping"] = json!(is_loop && boolean("Looping", true));
            let fade = number(
                if is_loop {
                    "Loop.FadeTimeIn"
                } else {
                    "FadeInTime"
                },
                0.0,
            );
            if fade > 0.0 {
                voice["envelopes"] =
                    json!([{"volume":[{"Time":0,"Value":0},{"Time":fade,"Value":1}],"offset":0}]);
            }
            if !is_loop {
                if let (Some(root), Some(wav)) =
                    (report["_source_root"].as_str(), voice["wav"].as_str())
                {
                    if let Ok(path) = relative(Path::new(root), wav) {
                        if let Ok(bytes) = read(&path, MAX_MEMBER_BYTES) {
                            intro_duration = delay
                                + audio_mix::duration(&bytes).ok()?
                                    / number("PitchTemplateMultiplier", 1.0);
                        }
                    }
                }
            }
        }
        voices.extend(branch);
    }
    if voices.is_empty() {
        // Interactive ensemble graphs (e.g. layered music) supply waves on
        // explicit Wave Player operators instead of template input arrays.
        // Select the exported solo layer, since multiplayer parameters do not
        // exist in this local playback system.
        let nodes = graph["PagedGraphs"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|p| p["Nodes"].as_array().into_iter().flatten());
        for node in nodes.filter(|n| n["Name"] == "Wave Player") {
            for value in node["InputLiterals"].as_array().into_iter().flatten() {
                if let Some(reference) = value["Value"]["AsUObject"]
                    .as_array()
                    .and_then(|a| a.first())
                {
                    if let Some(mut branch) =
                        resolve_wave(reference, report, warnings, &mut visited.clone(), depth + 1)
                    {
                        for voice in &mut branch {
                            voice["looping"] = json!(boolean("Looping", true));
                        }
                        warnings.push(
                            "Interactive MetaSound rendered with first exported solo wave layer"
                                .into(),
                        );
                        return Some(branch);
                    }
                }
            }
        }
        None
    } else {
        Some(voices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_graph_inputs_do_not_discard_available_music() {
        let wave = json!({"AssetPathName":"/Wave.Wave"});
        let report = json!({"Clips":[{"Asset":"/Wave.Wave","Playable":"wave.wav"}],"Graph":{
            "/Cue.Cue":{"Type":"SoundNodeMixer","Properties":{"ChildNodes":[null,wave],"InputVolume":[1,0.5]}},
            "/Meta.Meta":{"Type":"MetaSoundSource","Properties":{"RootMetasoundDocument":{"RootGraph":{"Interface":{"Inputs":[
                {"Name":"Music.Sounds","Defaults":[{"Literal":{"AsUObject":[null]}}]},
                {"Name":"Music.Loop.Sounds","Defaults":[{"Literal":{"AsUObject":[wave]}}]}
            ]}}}}}
        }});
        let mut warnings = Vec::new();
        let mixer = resolve_wave(
            &json!({"ObjectPath":"/Cue.Cue"}),
            &report,
            &mut warnings,
            &mut HashSet::new(),
            0,
        )
        .unwrap();
        assert_eq!(mixer.len(), 1);
        assert_eq!(mixer[0]["gain"], 0.5);
        let meta = resolve_wave(
            &json!({"ObjectPath":"/Meta.Meta"}),
            &report,
            &mut warnings,
            &mut HashSet::new(),
            0,
        )
        .unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0]["looping"], true);
    }
    #[test]
    fn resume_compares_the_persisted_float_representation() {
        let recipe = json!({"animation":{"retarget":{"ground_mesh":{"floor_z":-23.857952117919922f32 as f64,"point":3.11883282661438f32 as f64}}},"policy":4});
        let stored: Value =
            serde_json::from_slice(&serde_json::to_vec_pretty(&recipe).unwrap()).unwrap();
        assert!(matching_recipe(&stored, &recipe).unwrap());
        let mut changed = recipe.clone();
        changed["policy"] = json!(5);
        assert!(!matching_recipe(&stored, &changed).unwrap());
    }
    #[test]
    #[ignore = "requires DJK_FN_SOURCE with the extracted regression emotes"]
    fn laughtrack_corrective_and_driedsilk_have_audible_tracks() {
        let root = PathBuf::from(std::env::var("DJK_FN_SOURCE").unwrap());
        for (eid, stem) in [
            ("EID_LaughTrack", "LaughTrack_CMf"),
            ("EID_DriedSilk", "Emote_DriedSilk_CMF"),
            ("EID_Adapter", "Emote_Adapter_CMF"),
            ("EID_Accolades", "Emote_Accolades_CMF"),
        ] {
            let source = root.join(eid).canonicalize().unwrap();
            let psa = crate::psa::Psa::parse(
                &read(
                    &source.join(format!("Animations/{stem}.psa")),
                    MAX_MEMBER_BYTES,
                )
                .unwrap(),
            )
            .unwrap();
            let duration = psa.frames as f64 / psa.fps as f64;
            let mut warnings = Vec::new();
            let tracks = resolve_audio(&source, stem, duration, &mut warnings).unwrap();
            assert_eq!(
                tracks.len(),
                2,
                "missing 1P/3P tracks for {stem}: {warnings:?}"
            );
            assert!(
                !warnings.iter().any(|w| w.contains("Unresolved")),
                "{warnings:?}"
            );
            for track in tracks {
                let mix = &track["mix"];
                if eid == "EID_LaughTrack" {
                    assert_eq!(mix["voices"].as_array().unwrap().len(), 2);
                    assert!(mix["voices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v["looping"] == true));
                } else if eid == "EID_DriedSilk" {
                    assert!(mix["voices"].as_array().unwrap().len() > 3);
                    assert!(mix["voices"][0]["stop_seconds"].is_null());
                }
                let bytes = audio_mix::render(&source, mix).unwrap();
                assert!(
                    bytes[44..]
                        .chunks_exact(2)
                        .any(|b| i16::from_le_bytes(b.try_into().unwrap()).unsigned_abs() > 100),
                    "silent track for {stem}"
                );
            }
        }
    }
    #[test]
    fn graph_selects_a_branch_without_following_ownership_edges() {
        let report = json!({"Clips":[{"Asset":"/wave.wave","Playable":"chosen.wav"}],"Graph":{
            "/cue.Cue":{"Type":"SoundCue","Properties":{"FirstNode":{"ObjectName":"SoundNodeRandom'Cue:Random'","ObjectPath":"/cue.1"}}},
            "/cue.Cue:Random":{"Type":"SoundNodeRandom","Properties":{"ChildNodes":[{"ObjectPath":"/wave.0"},{"ObjectPath":"/other.0"}]}}
        }});
        let mut warnings = Vec::new();
        assert_eq!(
            resolve_wave(
                &json!({"ObjectName":"SoundCue'Cue'","ObjectPath":"/cue.0"}),
                &report,
                &mut warnings,
                &mut HashSet::new(),
                0
            ),
            Some(vec![
                json!({"wav":"Audio/chosen.wav","at_seconds":0.0,"gain":1.0,"pitch":1.0})
            ])
        );
        assert_eq!(warnings.len(), 1);
    }
    #[test]
    fn runtime_music_template_does_not_use_export_preview_delay() {
        let report = json!({"Clips":[{"Asset":"/wave.Wave","Playable":"intro.wav"}]});
        let mut props = json!({"RootMetasoundDocument":{"RootGraph":{
        "PagedGraphs":[{"Nodes":[{"Name":"MSS_Emote_Music1P"}]}],
        "Interface":{"Inputs":[
            {"Name":"Music.Sounds","Defaults":[{"Literal":{"AsUObject":[{"AssetPathName":"/wave.Wave"}]}}]},
            {"Name":"IntroDelay","Defaults":[{"Literal":{"AsFloat":[4.114]}}]}
        ]}}}});
        let resolve = |props: &Value| {
            resolve_metasound(props, &report, &mut Vec::new(), &mut HashSet::new(), 0).unwrap()
        };
        assert_eq!(resolve(&props)[0]["at_seconds"], 0.0);
        props["RootMetasoundDocument"]["RootGraph"]["PagedGraphs"][0]["Nodes"][0]["Name"] =
            json!("CustomAuthoredGraph");
        assert_eq!(resolve(&props)[0]["at_seconds"], 4.114);
    }
}
#[test]
fn licensed_switcher_uses_music_instead_of_the_exported_silence_placeholder() {
    let report = json!({"Clips":[{"Asset":"/silence.Silence","Playable":"5_seconds_silence.wav"},{"Asset":"/music.Music","Playable":"music.wav"}],"Graph":{"/cue.Switch":{"Type":"FortSoundNodeLicensedContentSwitcher","Properties":{"ChildNodes":[{"AssetPathName":"/silence.Silence"},{"AssetPathName":"/music.Music"}]}}}});
    let v = resolve_wave(
        &json!({"ObjectPath":"/cue.0","ObjectName":"FortSoundNodeLicensedContentSwitcher'Switch'"}),
        &report,
        &mut Vec::new(),
        &mut HashSet::new(),
        0,
    )
    .unwrap();
    assert_eq!(v[0]["wav"], "Audio/music.wav");
}
