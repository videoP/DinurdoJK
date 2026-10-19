//! Discover a source export's montage variations: read the item and playback
//! metadata the exporter wrote next to the PSA clips, follow section links and
//! random-section choices, and resolve each variation's audio. The result is the
//! `animation.variants` array that `montage::bake` composes.
//!
//! The input layout (`Item.json`, `PlaybackMetadata.json`, montage and notify
//! JSON next to `Animations/*.psa`) is what the Fortnite exporter produces; see
//! `EXPORT-AUDIT.md`.
use super::*;
use crate::psa::Psa;

pub fn definitions(
    root: &Path,
    clips: &[PathBuf],
    warnings: &mut Vec<String>,
) -> Result<Vec<Value>, String> {
    let metadata_path = root.join("PlaybackMetadata.json");
    let metadata = metadata_path
        .is_file()
        .then(|| super::batch::load_json(&metadata_path))
        .transpose()?;
    definitions_with_metadata(root, clips, warnings, metadata.as_ref())
}

pub fn definitions_with_metadata(
    root: &Path,
    clips: &[PathBuf],
    warnings: &mut Vec<String>,
    metadata: Option<&Value>,
) -> Result<Vec<Value>, String> {
    let item = super::batch::load_json(&root.join("Item.json"))?;
    let primary = item["Properties"]["Animation"]["AssetPathName"]
        .as_str()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .unwrap_or("");
    let mut montages = Vec::new();
    for file in fs::read_dir(root).map_err(|e| e.to_string())? {
        let file = file.map_err(|e| e.to_string())?.path();
        if file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            let v = super::batch::load_json(&file)?;
            if v["Type"] == "AnimMontage" {
                montages.push(v);
            }
        }
    }
    montages.sort_by_key(|m| {
        (
            m["Name"] != primary,
            m["Name"].as_str().unwrap_or("").to_owned(),
        )
    });
    let mut variants = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for mut m in montages {
        if let Some(packages) = metadata.and_then(|v| v["Packages"].as_object()) {
            if let Some((_, exports)) = packages.iter().find(|(p, _)| {
                m["Package"]
                    .as_str()
                    .is_some_and(|mp| mp.eq_ignore_ascii_case(p))
            }) {
                m["_PackageExports"] = exports.clone();
            }
        }
        let tracks = m["Properties"]["SlotAnimTracks"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let track = tracks
            .iter()
            .find(|t| {
                t["SlotName"] == "AdditiveCorrective"
                    && t["AnimTrack"]["AnimSegments"]
                        .as_array()
                        .is_some_and(|s| !s.is_empty())
            })
            .or_else(|| {
                tracks
                    .iter()
                    .find(|t| t["SlotName"] == "FullBody" || t["SlotName"] == "FullBodyNoAdditive")
            })
            .or_else(|| {
                tracks
                    .iter()
                    .find(|t| t["SlotName"] == "UpperBody" || t["SlotName"] == "DefaultSlot")
            })
            .or_else(|| {
                // Minifig/Sprout montages name their only body slot differently
                // (FullBodySlot, UpperBody_Prop1_Generic). Any populated body
                // slot is still the authored performance; additive layers and
                // mesh-rotation helpers are not.
                tracks.iter().find(|t| {
                    let slot = t["SlotName"].as_str().unwrap_or("");
                    !slot.contains("Additive")
                        && !slot.contains("Rotation")
                        && t["AnimTrack"]["AnimSegments"]
                            .as_array()
                            .is_some_and(|s| !s.is_empty())
                })
            });
        let Some(track) = track else {
            continue;
        };
        let mut segments = Vec::new();
        let mut missing = false;
        let full_body = tracks.iter().find(|t| t["SlotName"] == "FullBody");
        let full_segments = full_body.and_then(|t| t["AnimTrack"]["AnimSegments"].as_array());
        for s in track["AnimTrack"]["AnimSegments"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let name = super::batch::object_name(&s["AnimReference"]);
            let Some(clip) = clips.iter().find(|p| {
                p.file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&name)
            }) else {
                warnings.push(format!("{}: required animation track {name} is absent from the export; montage omitted", m["Name"].as_str().unwrap_or("Montage")));
                missing = true;
                break;
            };
            segments.push(json!({"psa":format!("Animations/{}",clip.file_name().unwrap().to_string_lossy()),
                "start":s["StartPos"].as_f64().unwrap_or(0.0),"from":s["AnimStartTime"].as_f64().unwrap_or(0.0),
                "to":s["AnimEndTime"],"rate":s["AnimPlayRate"].as_f64().unwrap_or(1.0),"repeats":s["LoopingCount"].as_u64().unwrap_or(1)}));
            // This exporter supplies the FIRST FullBody sequence to every
            // corrective segment. Later sections need that reference removed
            // and their own FullBody reference applied (never add twice).
            let metadata = root.join("Animations/Json").join(format!("{name}.json"));
            if metadata.is_file() {
                let value = super::batch::load_json(&metadata)?;
                if let Some(sequence) = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|v| v["Type"] == "AnimSequence")
                {
                    if let Some(length) = sequence["Properties"]["SequenceLength"]
                        .as_f64()
                        .filter(|n| n.is_finite() && *n > 0.0)
                    {
                        segments.last_mut().unwrap()["source_length_seconds"] = json!(length);
                    }
                    // FAnimSegment::GetValidPlayRate: AnimPlayRate * sequence RateScale.
                    // Montage section times already include it (Headset: 10.83 s
                    // sequence in a 10.41 s montage at RateScale 1.0409).
                    if let Some(scale) = sequence["Properties"]["RateScale"]
                        .as_f64()
                        .filter(|n| n.is_finite() && n.abs() > 1e-4 && (*n - 1.0).abs() > 1e-6)
                    {
                        let segment = segments.last_mut().unwrap();
                        let rate = segment["rate"].as_f64().unwrap_or(1.0) * scale;
                        segment["rate"] = json!(rate);
                    }
                }
            }
            if track["SlotName"] == "AdditiveCorrective" {
                if let Some(full) = full_segments {
                    let exported = full.first();
                    let correct = full.iter().find(|b| b["StartPos"] == s["StartPos"]);
                    if let (Some(exported), Some(correct)) = (exported, correct) {
                        let old_name = super::batch::object_name(&exported["AnimReference"]);
                        let new_name = super::batch::object_name(&correct["AnimReference"]);
                        if old_name != new_name {
                            let metadata =
                                root.join("Animations/Json").join(format!("{name}.json"));
                            if metadata.is_file() {
                                let meta = super::batch::load_json(&metadata)?;
                                let props = meta
                                    .as_array()
                                    .and_then(|a| a.iter().find(|a| a["Type"] == "AnimSequence"))
                                    .map(|a| &a["Properties"])
                                    .unwrap_or(&meta["Properties"]);
                                let already_correct = meta
                                    .as_array()
                                    .and_then(|a| a.first())
                                    .and_then(|v| v["DjkExportAdditiveReference"].as_str())
                                    .is_some_and(|n| n.eq_ignore_ascii_case(&new_name));
                                if !already_correct
                                    && props["AdditiveAnimType"]
                                        == "EAdditiveAnimationType::AAT_LocalSpaceBase"
                                    && props["RefPoseType"]
                                        == "EAdditiveBasePoseType::ABPT_AnimScaled"
                                {
                                    let lookup = |n: &str| {
                                        clips
                                            .iter()
                                            .find(|p| {
                                                p.file_stem()
                                                    .unwrap()
                                                    .to_string_lossy()
                                                    .eq_ignore_ascii_case(n)
                                            })
                                            .map(|p| {
                                                format!(
                                                    "Animations/{}",
                                                    p.file_name().unwrap().to_string_lossy()
                                                )
                                            })
                                    };
                                    if let (Some(old), Some(new)) =
                                        (lookup(&old_name), lookup(&new_name))
                                    {
                                        segments.last_mut().unwrap()["repair_additive_reference"] =
                                            json!({"exported":old,"correct":new});
                                        warnings.push(format!("Repaired {name}: exporter used {old_name} instead of {new_name} as additive reference"));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if missing || segments.is_empty() {
            continue;
        }
        let duration = m["Properties"]["SequenceLength"].as_f64().unwrap_or(0.0);
        if duration <= 0.0 || duration > 3600.0 {
            continue;
        }
        // A choice is an entry section followed by its authored transitions;
        // Loop/Exit connectors are never independent selectable animations.
        for path in section_paths(&m, &item["Properties"], warnings)? {
            let spans = path.spans;
            let at = path.duration;
            let loop_start = path.loop_start;
            if at <= 0.0 {
                continue;
            }
            let name = m["Name"].as_str().unwrap_or("Montage");
            let mut audio_warnings = Vec::new();
            let audio = super::batch::resolve_montage_audio(
                root,
                name,
                &segments,
                &spans,
                at,
                loop_start,
                &mut audio_warnings,
            )?;
            // Equal motion can still be a different authored sound choice.
            let signature =
                serde_json::to_string(&json!([segments, spans, loop_start, audio])).unwrap();
            if !seen.insert(signature) {
                continue;
            }
            let (status, detail) = audio_diagnostic(root, &audio, &audio_warnings)?;
            warnings.extend(audio_warnings);
            let display_name = path
                .choice
                .map_or_else(|| name.to_owned(), |choice| format!("{name} / {choice}"));
            variants.push(json!({"name":display_name,"montage":name,"segments":segments,"spans":spans,"duration_seconds":at,"loop_start_seconds":loop_start,"audio":audio,"audio_status":status,"audio_detail":detail}));
        }
    }
    if variants.is_empty() {
        // Truly standalone sequences remain usable, grouped into their item's variants.
        for clip in clips {
            let psa = Psa::load(clip)?;
            let duration = psa.frames as f64 / psa.fps as f64;
            let name = clip.file_stem().unwrap().to_string_lossy();
            let audio = super::batch::resolve_audio(root, &name, duration, warnings)?;
            let (status, detail) = audio_diagnostic(root, &audio, warnings)?;
            variants.push(json!({"name":name,"segments":[{"psa":format!("Animations/{}",clip.file_name().unwrap().to_string_lossy()),"start":0.0,"from":0.0,"to":duration,"rate":1.0,"repeats":1}],"spans":[{"from":0.0,"to":duration,"at":0.0}],"duration_seconds":duration,"audio":audio,"audio_status":status,"audio_detail":detail}));
        }
        warnings.push(
            "No playable full-body montage exported; standalone sequence variants retained".into(),
        );
    }
    Ok(variants)
}

struct SectionPath {
    choice: Option<String>,
    spans: Vec<Value>,
    duration: f64,
    loop_start: Option<f64>,
}

#[derive(Clone)]
struct SectionController {
    section: usize,
    after: usize,
    targets: Vec<usize>,
}

fn section_paths(
    montage: &Value,
    item: &Value,
    warnings: &mut Vec<String>,
) -> Result<Vec<SectionPath>, String> {
    let duration = montage["Properties"]["SequenceLength"]
        .as_f64()
        .ok_or("missing montage duration")?;
    let mut sections = montage["Properties"]["CompositeSections"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    sections.sort_by(|a, b| {
        a["LinkValue"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&b["LinkValue"].as_f64().unwrap_or(0.0))
    });
    if sections.is_empty() {
        return Ok(vec![SectionPath {
            choice: None,
            spans: vec![json!({"from":0.0,"to":duration,"at":0.0})],
            duration,
            loop_start: None,
        }]);
    }
    let name = |index: usize| sections[index]["SectionName"].as_str().unwrap_or("");
    let lookup = |n: &str| {
        sections.iter().position(|s| {
            s["SectionName"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(n))
        })
    };
    let mut controllers = Vec::new();
    for notify in montage["Properties"]["Notifies"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let reference = super::batch::object_name(&notify["NotifyStateClass"]);
        let Some(obj) = montage["_PackageExports"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| {
                o["Type"] == "FortAnimNotifyState_RandomizeNextSection"
                    && o["Name"].as_str() == reference.rsplit(':').next()
            })
        else {
            if reference.contains("RandomizeNextSection") {
                warnings.push("Missing RandomizeNextSection package metadata; export PlaybackMetadata.json to preserve loop connectors".into());
            }
            continue;
        };
        let time = notify["LinkValue"].as_f64().unwrap_or(0.0);
        let section = sections
            .iter()
            .rposition(|s| s["LinkValue"].as_f64().unwrap_or(0.0) <= time + 0.0001)
            .unwrap_or(0);
        let props = &obj["Properties"];
        let targets: Vec<_> = props["SectionsToJumpTo"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().and_then(lookup))
            .collect();
        if targets.is_empty() {
            warnings.push(format!(
                "{}: connector notify has no available target section",
                montage["Name"]
            ));
            continue;
        }
        let min = props["MinLoopsBeforeChange"]
            .as_u64()
            .unwrap_or(1)
            .clamp(1, 16) as usize;
        let max = props["MaxLoopsBeforeChange"]
            .as_u64()
            .unwrap_or(min as u64)
            .clamp(min as u64, 16) as usize;
        let after = (min + max) / 2;
        if min != max || targets.len() > 1 {
            warnings.push(format!("{}: baked connector choices in source order after {after} loops (source range {min}..{max}); runtime random choice becomes a repeatable cycle",montage["Name"]));
        }
        controllers.push(SectionController {
            section,
            after,
            targets,
        });
    }
    let mut entries = Vec::new();
    for row in item["PlayRandomSectionByName"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if row["SectionWeight"].as_f64().unwrap_or(1.0) <= 0.0 {
            continue;
        }
        if let Some(index) = row["SectionName"].as_str().and_then(lookup) {
            if !entries.contains(&index) {
                entries.push(index);
            }
        }
    }
    if entries.is_empty() {
        entries.push(0);
    }
    for row in item["MotageSelectionGroups"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(index) = row["MontageSectionName"].as_str().and_then(lookup) {
            if !entries.contains(&index) {
                entries.push(index);
            }
        }
    }
    if item["bPlayRandomSection"] == true
        && item["PlayRandomSectionByName"]
            .as_array()
            .is_none_or(Vec::is_empty)
    {
        // Unnamed random choices start at graph roots. Exclude a section reached
        // from another section, so an intro's loop/connector stays attached.
        for index in 0..sections.len() {
            if !sections.iter().enumerate().any(|(j, s)| {
                j != index
                    && s["NextSectionName"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(name(index)))
            }) && !entries.contains(&index)
            {
                entries.push(index);
            }
        }
    }
    let choices = entries.len() > 1;
    let mut output = Vec::new();
    for mut index in entries {
        let choice = choices.then(|| name(index).to_owned());
        let mut visited = std::collections::HashMap::new();
        let mut spans = Vec::new();
        let mut at = 0.0;
        let mut loop_start = None;
        let mut counters = vec![(0usize, 0usize); controllers.len()];
        loop {
            let state = (index, counters.clone());
            if let Some(start) = visited.get(&state) {
                loop_start = Some(*start);
                break;
            }
            if visited.len() >= 1024 || at > 3600.0 {
                return Err("connector cycle exceeds compiler limit".into());
            }
            visited.insert(state, at);
            let start = sections[index]["LinkValue"].as_f64().unwrap_or(0.0);
            let end = sections
                .get(index + 1)
                .and_then(|s| s["LinkValue"].as_f64())
                .unwrap_or(duration);
            if !start.is_finite()
                || !end.is_finite()
                || start < 0.0
                || end > duration + 0.0001
                || end <= start
            {
                return Err(format!(
                    "invalid montage section {}: {start}..{end}",
                    name(index)
                ));
            }
            spans.push(json!({"from":start,"to":end,"at":at}));
            at += end - start;
            let mut jump = None;
            for (controller, counter) in controllers.iter().zip(&mut counters) {
                if controller.section == index {
                    counter.0 += 1;
                    if counter.0 >= controller.after {
                        jump = Some(controller.targets[counter.1]);
                        counter.0 = 0;
                        counter.1 = (counter.1 + 1) % controller.targets.len();
                        break;
                    }
                }
            }
            if let Some(next_index) = jump {
                index = next_index;
                continue;
            }
            let next = sections[index]["NextSectionName"]
                .as_str()
                .unwrap_or("None");
            if next.eq_ignore_ascii_case("None") {
                break;
            }
            let Some(next_index) = lookup(next) else {
                warnings.push(format!(
                    "{}: section {} points to unavailable section {next}",
                    montage["Name"].as_str().unwrap_or("Montage"),
                    name(index)
                ));
                break;
            };
            index = next_index;
        }
        output.push(SectionPath {
            choice,
            spans,
            duration: at,
            loop_start,
        });
    }
    Ok(output)
}

fn audio_diagnostic(
    root: &Path,
    audio: &[Value],
    warnings: &[String],
) -> Result<(String, String), String> {
    let file = root.join("Audio/AudioReport.json");
    let report = if file.is_file() {
        super::batch::load_json(&file)?
    } else {
        Value::Null
    };
    let clips = report["Clips"].as_array().map_or(0, |a| {
        a.iter()
            .filter(|c| c["Playable"].as_str().is_some())
            .count()
    });
    let problems: Vec<_> = warnings
        .iter()
        .filter(|w| w.contains("Unresolved") || w.contains("Unscheduled"))
        .cloned()
        .collect();
    let errors = report["Errors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if audio.is_empty() {
        if clips == 0 {
            Ok(("missing_source".into(),format!("No decoded source WAVs in this export. Re-export after obtaining the missing source packages.\n{errors}")))
        } else {
            Ok(("unmapped".into(),format!("{clips} decoded source WAVs exist, but this variation has no resolved sound event. Investigate its notify/graph mapping.\n{}\n{errors}",problems.join("\n"))))
        }
    } else if !problems.is_empty() {
        Ok(("partial".into(), problems.join("\n")))
    } else if !audio.iter().any(|e| e["lane"] == "music") {
        Ok((
            "foley_only".into(),
            "Only sound effects were resolved; no music track is scheduled for this variation."
                .into(),
        ))
    } else {
        Ok((
            "available".into(),
            "Music events resolved from exported source metadata.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::montage::{compose, repair_additive_reference};
    use crate::psa::Key;
    #[test]
    fn section_names_follow_unreal_case_insensitive_names() {
        let montage = json!({"Name":"Case","Properties":{"SequenceLength":3.0,"CompositeSections":[{"SectionName":"Intro","NextSectionName":"lOOP","LinkValue":0.0},{"SectionName":"Loop","NextSectionName":"LOOP","LinkValue":1.0}]}});
        let paths = section_paths(&montage, &json!({}), &mut Vec::new()).unwrap();
        assert_eq!(paths[0].duration, 3.0);
        assert_eq!(paths[0].loop_start, Some(1.0));
    }
    #[test]
    fn explicit_choices_keep_their_connectors_attached() {
        let montage = json!({"Name":"Choice","Properties":{"SequenceLength":6.0,"CompositeSections":[{"SectionName":"IntroA","NextSectionName":"LoopA","LinkValue":0.0},{"SectionName":"LoopA","NextSectionName":"LoopA","LinkValue":1.0},{"SectionName":"IntroB","NextSectionName":"LoopB","LinkValue":3.0},{"SectionName":"LoopB","NextSectionName":"LoopB","LinkValue":4.0}]}});
        let paths=section_paths(&montage,&json!({"PlayRandomSectionByName":[{"SectionName":"IntroA","SectionWeight":1},{"SectionName":"IntroB","SectionWeight":1}]}),&mut Vec::new()).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths
            .iter()
            .all(|p| p.spans.len() == 2 && p.duration == 3.0 && p.loop_start == Some(1.0)));
    }
    #[test]
    fn fourth_loop_connector_returns_to_the_main_loop() {
        let montage = json!({"Name":"Fourth","Properties":{"SequenceLength":4.0,"CompositeSections":[{"SectionName":"Intro","NextSectionName":"Loop","LinkValue":0.0},{"SectionName":"Loop","NextSectionName":"Loop","LinkValue":1.0},{"SectionName":"Connector","NextSectionName":"Loop","LinkValue":3.0}],"Notifies":[{"LinkValue":1.0,"NotifyStateClass":{"ObjectName":"FortAnimNotifyState_RandomizeNextSection'Fourth:Controller'"}}]},"_PackageExports":[{"Type":"FortAnimNotifyState_RandomizeNextSection","Name":"Controller","Properties":{"MinLoopsBeforeChange":3,"MaxLoopsBeforeChange":3,"SectionsToJumpTo":["Connector"]}}]});
        let paths = section_paths(&montage, &json!({}), &mut Vec::new()).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].duration, 8.0);
        assert_eq!(paths[0].loop_start, Some(1.0));
        assert_eq!(
            paths[0]
                .spans
                .iter()
                .map(|s| s["from"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            vec![0.0, 1.0, 1.0, 1.0, 3.0]
        );
    }
    #[test]
    #[ignore = "requires DJK_FN_SOURCE with exported Worm and Floss assets"]
    fn worm_and_floss_use_sequence_endpoints_and_independent_music_loops() {
        let source = PathBuf::from(std::env::var("DJK_FN_SOURCE").unwrap());
        for id in [
            "EID_Worm",
            "EID_Floss",
            "EID_OneArmFloss",
            "EID_WindmillFloss",
        ] {
            let root = source.join(id).canonicalize().unwrap();
            let clips: Vec<_> = fs::read_dir(root.join("Animations"))
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "psa"))
                .collect();
            let variants = definitions(&root, &clips, &mut Vec::new()).unwrap();
            for v in variants {
                let p = compose(&root, &v).unwrap();
                let loop_start = v["loop_start_seconds"].as_f64().unwrap();
                if id == "EID_Worm" {
                    assert!((loop_start - 2.2195237).abs() < 1e-5);
                    assert_eq!(p.frames, 84); // End section never plays while looping.
                    let segment = &v["segments"][0];
                    let raw = Psa::parse(
                        &read(
                            &root.join(segment["psa"].as_str().unwrap()),
                            MAX_MEMBER_BYTES,
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let f = loop_start * (raw.frames - 1) as f64
                        / segment["source_length_seconds"].as_f64().unwrap();
                    let a = f.floor() as usize;
                    for (bone, _) in p.bones.iter().enumerate() {
                        let expected = raw.keys[a * raw.bones.len() + bone].position.lerp(
                            raw.keys[(a + 1) * raw.bones.len() + bone].position,
                            f.fract() as f32,
                        );
                        assert!(
                            (p.keys[67 * p.bones.len() + bone].position - expected).length()
                                < 0.001
                        );
                    }
                }
                let music: Vec<_> = v["audio"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|e| e["lane"] == "music")
                    .collect();
                assert!(
                    music.iter().any(|e| e["loop_start_seconds"].is_number()
                        && e["mix"]["duration_seconds"]
                            .as_f64()
                            .is_some_and(|n| n > 4.0)),
                    "{id}: {music:?}"
                );
            }
        }
    }
    use super::*;
    #[test]
    fn additive_repair_removes_the_exported_base_before_applying_the_loop() {
        use glam::{Quat, Vec3};
        let make = |rotation, position, scale| Psa {
            bones: vec![crate::psa::Bone {
                name: "root".into(),
                parent: None,
                rest_rotation: Quat::IDENTITY,
                rest_position: Vec3::ZERO,
            }],
            fps: 30.0,
            frames: 1,
            keys: vec![Key { rotation, position }],
            scales: vec![scale],
        };
        let old = make(Quat::from_rotation_x(0.4), Vec3::X, Vec3::splat(2.0));
        let correct = make(Quat::from_rotation_y(0.8), Vec3::Y * 3.0, Vec3::splat(4.0));
        let correction = Quat::from_rotation_z(-0.3);
        let mut exported = make(
            correction * old.keys[0].rotation,
            old.keys[0].position + Vec3::Z,
            Vec3::splat(3.0),
        );
        repair_additive_reference(&mut exported, &old, &correct).unwrap();
        assert!(
            exported.keys[0]
                .rotation
                .angle_between(correction * correct.keys[0].rotation)
                < 0.001
        );
        assert_eq!(exported.keys[0].position, Vec3::Y * 3.0 + Vec3::Z);
        assert_eq!(exported.scales[0], Vec3::splat(6.0));
    }
    #[test]
    fn additive_repair_keeps_hidden_helpers_collapsed_without_dividing_by_zero() {
        use glam::{Quat, Vec3};
        let make = || Psa {
            bones: vec![crate::psa::Bone {
                name: "weapon_r".into(),
                parent: None,
                rest_rotation: Quat::IDENTITY,
                rest_position: Vec3::ZERO,
            }],
            fps: 30.0,
            frames: 1,
            keys: vec![Key {
                rotation: Quat::IDENTITY,
                position: Vec3::ZERO,
            }],
            scales: vec![Vec3::ZERO],
        };
        let mut clip = make();
        repair_additive_reference(&mut clip, &make(), &make()).unwrap();
        assert_eq!(clip.scales[0], Vec3::ZERO);
    }
    #[test]
    #[ignore = "requires DJK_FN_SOURCE with exported Baller assets"]
    fn baller_uses_its_own_loop_instead_of_repeating_the_intro_reference() {
        let root = PathBuf::from(std::env::var("DJK_FN_SOURCE").unwrap())
            .join("EID_Basketball")
            .canonicalize()
            .unwrap();
        let clips = fs::read_dir(root.join("Animations"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "psa"))
            .collect::<Vec<_>>();
        let variants = definitions(&root, &clips, &mut Vec::new()).unwrap();
        let male = variants
            .iter()
            .find(|v| v["name"] == "Basketball_CMM")
            .unwrap();
        assert!(male["segments"][1]["repair_additive_reference"].is_object());
        assert!((male["loop_start_seconds"].as_f64().unwrap() - 1.0 / 3.0).abs() < 0.001);
        let composed = compose(&root, male).unwrap();
        let actual = Psa::parse(
            &read(
                &root.join("Animations/Basketball_Loop_CMM_30fps.psa"),
                MAX_MEMBER_BYTES,
            )
            .unwrap(),
        )
        .unwrap();
        let pelvis = composed
            .bones
            .iter()
            .position(|b| b.name == "pelvis")
            .unwrap();
        for f in [20, 50, 100] {
            let a = composed.keys[(f + 10) * composed.bones.len() + pelvis];
            let length = male["segments"][1]["source_length_seconds"]
                .as_f64()
                .unwrap();
            let sample = f as f64 / 30.0 * (actual.frames - 1) as f64 / length;
            let first = sample.floor() as usize;
            let x = actual.keys[first * actual.bones.len() + pelvis];
            let y = actual.keys[(first + 1) * actual.bones.len() + pelvis];
            let b = Key {
                position: x.position.lerp(y.position, sample.fract() as f32),
                rotation: x.rotation.slerp(y.rotation, sample.fract() as f32),
            };
            assert!(
                (a.position - b.position).length() < 0.01,
                "frame {f}: {:?} vs {:?}",
                a.position,
                b.position
            );
            assert!(a.rotation.angle_between(b.rotation) < 0.01);
        }
        for audience in ["1p", "3p"] {
            assert!(male["audio"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["lane"] == "music"
                    && a["audience"] == audience
                    && (a["repeat_on_loop"] == true
                        || a["loop_start_seconds"]
                            .as_f64()
                            .is_some_and(|start| a["mix"]["duration_seconds"]
                                .as_f64()
                                .is_some_and(|end| end > start)))));
        }
    }
    #[test]
    #[ignore = "requires DJK_FN_SOURCE and DJK_FN_METADATA with recovered BackPlates packages"]
    fn recovered_backplates_notify_inserts_the_fourth_loop_connector() {
        let root = PathBuf::from(std::env::var("DJK_FN_SOURCE").unwrap())
            .join("EID_BackPlates")
            .canonicalize()
            .unwrap();
        let metadata = super::batch::load_json(
            &PathBuf::from(std::env::var("DJK_FN_METADATA").unwrap()).join("EID_BackPlates.json"),
        )
        .unwrap();
        let clips = fs::read_dir(root.join("Animations"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "psa"))
            .collect::<Vec<_>>();
        for variant in
            definitions_with_metadata(&root, &clips, &mut Vec::new(), Some(&metadata)).unwrap()
        {
            let spans = variant["spans"].as_array().unwrap();
            assert_eq!(
                spans
                    .iter()
                    .filter(|s| (s["from"].as_f64().unwrap() - 1.0333333).abs() < 0.001)
                    .count(),
                3
            );
            assert!(spans
                .iter()
                .any(|s| (s["from"].as_f64().unwrap() - 9.566667).abs() < 0.001));
            assert!((variant["loop_start_seconds"].as_f64().unwrap() - 1.0333333).abs() < 0.001);
            let composed = compose(&root, &variant).unwrap();
            assert!(
                (composed.frames as f64 / 30.0 - variant["duration_seconds"].as_f64().unwrap())
                    .abs()
                    < 0.04
            );
        }
    }
    #[test]
    #[ignore = "requires DJK_FN_SOURCE with exported Fortnite assets"]
    fn exported_montages_group_variants_and_preserve_intro_loop_audio() {
        let source = PathBuf::from(std::env::var("DJK_FN_SOURCE").unwrap());
        for eid in [
            "EID_LaughTrack",
            "EID_DriedSilk",
            "EID_DanceMoves",
            "EID_AccentWall",
            "EID_Annual",
        ] {
            let root = source.join(eid).canonicalize().unwrap();
            let clips = fs::read_dir(root.join("Animations"))
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "psa"))
                .collect::<Vec<_>>();
            let mut warnings = Vec::new();
            let variants = definitions(&root, &clips, &mut warnings).unwrap();
            assert!(!variants.is_empty());
            for variant in &variants {
                let clip = compose(&root, variant).unwrap();
                assert_eq!(clip.fps, 30.0);
                assert!(
                    (clip.frames as f64 / 30.0 - variant["duration_seconds"].as_f64().unwrap())
                        .abs()
                        < 0.034
                );
                assert!(
                    variant["audio"].as_array().is_some_and(|v| !v.is_empty()),
                    "{eid} {}: {warnings:?}",
                    variant["name"]
                );
                for track in variant["audio"].as_array().unwrap() {
                    let bytes = audio_mix::render(&root, &track["mix"]).unwrap();
                    assert!(
                        bytes[44..]
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .any(|p| i16::from_le_bytes(*p).abs() > 100),
                        "silent {eid}"
                    );
                }
            }
            if eid == "EID_LaughTrack" {
                assert!(variants[0]["loop_start_seconds"].as_f64().unwrap() > 2.0);
            }
            if eid == "EID_Annual" {
                assert!(variants
                    .iter()
                    .any(|v| v["segments"].as_array().unwrap().len() > 1));
            }
        }
    }
}
