//! Rebuild source contact heights while retaining shipped audio and timelines.
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub fn run(
    source: &Path,
    authoring: &Path,
    input: &Path,
    base: &Path,
    output: &Path,
    jobs: usize,
    filter: Option<&str>,
) -> Result<(), String> {
    if output.exists() {
        return Err("output already exists".into());
    }
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    let target = Arc::new(jka_assets::ghoul2::parse_gla(&target_from_model(
        base,
        DEFAULT_TARGET_MODEL,
    )?)?);
    let ground = ground_mesh(base, &target, DEFAULT_TARGET_MODEL)?;
    let mut files = fs::read_dir(input)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("djk_fnemote"))
        })
        .filter(|p| {
            filter.is_none_or(|f| {
                f.split(',')
                    .any(|id| p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(id)))
            })
        })
        .collect::<Vec<_>>();
    files.sort();
    if files.is_empty() {
        return Err("no matching bundles".into());
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, 8) {
            let tx = tx.clone();
            let files = &files;
            let next = &next;
            let target = &target;
            let ground = &ground;
            let source = &source;
            scope.spawn(move || loop {
                let n = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(n) else {
                    break;
                };
                let result = reground(source, authoring, path, output, &target, ground);
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
                println!("Grounded {done}/{} ({} errors)", files.len(), errors.len());
            }
        }
        records.sort_by_key(|r| r["id"].as_str().unwrap_or("").to_owned());
        fs::write(
            output.join("ground-contact-report.json"),
            serde_json::to_vec_pretty(&json!({"bundles":records,"errors":errors})).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("{} bundles failed", errors.len()))
        }
    })
}

fn reground(
    source: &Path,
    authoring: &Path,
    path: &Path,
    output: &Path,
    target: &jka_assets::ghoul2::GlaAnimation,
    ground: &Value,
) -> Result<Value, String> {
    let shipped = Bundle::open(path)?;
    let original = shipped.pose.as_ref().ok_or("missing pose track")?;
    let author = Bundle::open_index(&authoring.join(path.file_name().unwrap()))?;
    let mut recipe: Value =
        serde_json::from_slice(&author.read("metadata/recipe.json", MAX_MEMBER_BYTES)?)
            .map_err(|e| e.to_string())?;
    if recipe["animation"]["target_model"]
        .as_str()
        .unwrap_or(DEFAULT_TARGET_MODEL)
        != DEFAULT_TARGET_MODEL
    {
        return Err(
            "model differs from this library's target; use compile with its selected model".into(),
        );
    }
    recipe["animation"]["retarget"]["ground_mesh"] = ground.clone();
    let mut root = shipped.manifest.clone();
    let id = root["emotes"][0]["id"]
        .as_str()
        .ok_or("missing ID")?
        .to_owned();
    let (_, raw, report) = montage::bake(&source.join(&id), &recipe["animation"], target)?;
    let new = crate::PoseClip::parse(&raw)?;
    if (new.frames, new.bones) != (original.frames, original.bones) {
        return Err("pose timeline changed".into());
    }
    let old_rows = root["emotes"][0]["variants"]
        .as_array()
        .ok_or("missing variants")?;
    let new_rows = report["variants"]
        .as_array()
        .ok_or("missing baked variants")?;
    if old_rows.len() != new_rows.len() {
        return Err("variant count changed".into());
    }
    for (a, b) in old_rows.iter().zip(new_rows) {
        for key in [
            "first_frame",
            "end_frame",
            "fps",
            "loop",
            "loop_start_frame",
        ] {
            if a[key] != b[key] {
                return Err(format!("variant {key} changed"));
            }
        }
    }
    // This migration changes only vertical translation, never the solved joints.
    for (a, b) in original.matrices().iter().zip(new.matrices()) {
        for r in 0..3 {
            for c in 0..4 {
                if (r, c) != (2, 3) && (a[r][c] - b[r][c]).abs() > 0.00051 {
                    return Err("non-height pose coefficient changed".into());
                }
            }
        }
    }
    let packed = crate::PoseClip::pack_compact(&raw)?;
    let mut members = BTreeMap::from([
        (
            "animation.gla".to_owned(),
            shipped.read("animation.gla", MAX_MEMBER_BYTES)?,
        ),
        ("animation.pose".to_owned(), packed),
    ]);
    for e in crate::manifest::parse_manifest(&root)? {
        for a in e
            .audio
            .iter()
            .chain(e.variants.iter().flat_map(|v| &v.audio))
        {
            if let Some(name) = &a.wav {
                if !members.contains_key(name) {
                    members.insert(name.clone(), shipped.read(name, crate::MAX_WAV_BYTES)?);
                }
            }
        }
    }
    root["ground_contact_method"] = json!("source-contact-curve-v1");
    crate::write_shipping_bundle(&output.join(path.file_name().unwrap()), &root, &members)?;
    Ok(
        json!({"id":id,"frames":new.frames,"variants":report["variants"].as_array().unwrap().len(),
        "bytes":fs::metadata(output.join(path.file_name().unwrap())).map_err(|e|e.to_string())?.len()}),
    )
}
