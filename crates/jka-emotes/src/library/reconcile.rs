//! Restore section choices/connectors while copying unaffected shipping bundles.
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub fn run(
    source: &Path,
    metadata: &Path,
    authoring: &Path,
    input: &Path,
    base: &Path,
    output: &Path,
    jobs: usize,
) -> Result<(), String> {
    if output.exists() {
        return Err("use a fresh output directory".into());
    }
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    let target = Arc::new(jka_assets::ghoul2::parse_gla(&target_from_model(
        base,
        DEFAULT_TARGET_MODEL,
    )?)?);
    let ground = ground_mesh(base, &target, DEFAULT_TARGET_MODEL)?;
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
    fs::create_dir_all(output.join("authoring")).map_err(|e| e.to_string())?;
    fs::create_dir_all(output.join("exact")).map_err(|e| e.to_string())?;
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, 8) {
            let tx = tx.clone();
            let files = &files;
            let next = &next;
            let source = &source;
            let target = &target;
            let ground = &ground;
            scope.spawn(move || loop {
                let n = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(n) else { break };
                let result = reconcile(source, metadata, authoring, path, output, target, ground);
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
                Ok(v) => records.push(v),
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
            let done = records.len() + errors.len();
            if done % 25 == 0 || done == files.len() {
                println!(
                    "Reconciled {done}/{} ({} errors)",
                    files.len(),
                    errors.len()
                );
            }
        }
        records.sort_by_key(|r| r["id"].as_str().unwrap_or("").to_owned());
        let links = crate::library::catalog::index_source(&source)?;
        fs::write(
            output.join("catalog-links.json"),
            serde_json::to_vec_pretty(&links).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        fs::write(
            output.join("reconciliation-report.json"),
            serde_json::to_vec_pretty(&json!({"bundles":records,"errors":errors})).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{} bundles failed; see reconciliation-report.json",
                errors.len()
            ))
        }
    })
}

fn reconcile(
    source: &Path,
    metadata: &Path,
    authoring: &Path,
    path: &Path,
    output: &Path,
    target: &jka_assets::ghoul2::GlaAnimation,
    ground: &Value,
) -> Result<Value, String> {
    let shipped = Bundle::open_index(path)?;
    let id = text(&shipped.manifest["emotes"][0], "id")?;
    let overlay = metadata.join("Source").join(id);
    let root = if overlay.join("ExtraMontages.json").is_file() {
        overlay.canonicalize().map_err(|e| e.to_string())?
    } else {
        source.join(id)
    };
    let supplement = batch::load_json(&metadata.join(format!("{id}.json")))?;
    let item = batch::load_json(&root.join("Item.json"))?;
    let mut reasons = Vec::new();
    if root.join("ExtraMontages.json").is_file() {
        reasons.push(json!({"montage_swaps":true}));
    }
    for package in supplement["Packages"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, v)| v)
    {
        for m in package
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v["Type"] == "AnimMontage")
        {
            let sections = m["Properties"]["CompositeSections"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let case_fix = sections.iter().any(|s| {
                s["NextSectionName"].as_str().is_some_and(|n| {
                    !n.eq_ignore_ascii_case("None")
                        && !sections.iter().any(|s| s["SectionName"] == n)
                        && sections.iter().any(|s| {
                            s["SectionName"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(n))
                        })
                })
            });
            let choice = ["PlayRandomSectionByName", "MotageSelectionGroups"]
                .iter()
                .any(|key| {
                    item["Properties"][key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|r| {
                            r[if *key == "PlayRandomSectionByName" {
                                "SectionName"
                            } else {
                                "MontageSectionName"
                            }]
                            .as_str()
                            .is_some_and(|n| {
                                sections.iter().skip(1).any(|s| {
                                    s["SectionName"]
                                        .as_str()
                                        .is_some_and(|s| s.eq_ignore_ascii_case(n))
                                })
                            })
                        })
                });
            let controller = package
                .as_array()
                .into_iter()
                .flatten()
                .any(|o| o["Type"] == "FortAnimNotifyState_RandomizeNextSection");
            if case_fix || choice || controller || item["Properties"]["bPlayRandomSection"] == true
            {
                reasons.push(json!({"montage":m["Name"],"case_fix":case_fix,"choices":choice,"connector_notify":controller}));
            }
        }
    }
    let destination = output.join(path.file_name().unwrap());
    if reasons.is_empty() {
        fs::copy(path, &destination).map_err(|e| e.to_string())?;
        return Ok(
            json!({"id":id,"status":"unchanged","variants":shipped.manifest["emotes"][0]["variants"].as_array().map_or(1,Vec::len)}),
        );
    }
    let original = Bundle::open_index(&authoring.join(path.file_name().unwrap()))?;
    let mut recipe: Value =
        serde_json::from_slice(&original.read("metadata/recipe.json", MAX_MEMBER_BYTES)?)
            .map_err(|e| e.to_string())?;
    if recipe["animation"]["target_model"]
        .as_str()
        .unwrap_or(DEFAULT_TARGET_MODEL)
        != DEFAULT_TARGET_MODEL
    {
        return Err("selected target differs from migration rig".into());
    }
    let mut clips = fs::read_dir(root.join("Animations"))
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    clips.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("psa")));
    clips.sort();
    let mut warnings = Vec::new();
    recipe["animation"]["variants"] = json!(discovery::definitions_with_metadata(
        &root,
        &clips,
        &mut warnings,
        Some(&supplement)
    )?);
    recipe["animation"]["retarget"]["ground_mesh"] = ground.clone();
    recipe["source_root"] = json!(root);
    recipe["include_compressed_sources"] = json!(false);
    let author = output.join("authoring").join(path.file_name().unwrap());
    compile_recipe(&recipe, &root, &author, Some(target))?;
    let exact = output.join("exact").join(path.file_name().unwrap());
    shipping::pack(
        root.parent().ok_or("missing source parent")?,
        &author,
        &exact,
        "small",
    )?;
    shipping::pack(
        root.parent().ok_or("missing source parent")?,
        &exact,
        &destination,
        "tiny",
    )?;
    let result = Bundle::open_index(&destination)?;
    Ok(
        json!({"id":id,"status":"rebuilt","reasons":reasons,"warnings":warnings,"variants":result.manifest["emotes"][0]["variants"].as_array().map_or(1,Vec::len)}),
    )
}
