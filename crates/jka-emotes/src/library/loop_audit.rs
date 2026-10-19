//! Measure loop seams of compiled emote variants.
//!
//! For each looping variant the pose jump from the last frame back to the
//! loop-start frame is compared with that variant's ordinary frame-to-frame
//! motion. A seam far above the variant's own motion is a visible pop.
use crate::Bundle;
use serde_json::{json, Value};
use std::path::Path;

type M = jka_assets::ghoul2::Matrix3x4;

/// Largest bone displacement (game units) between two poses, measured on a
/// probe point 8 units along each bone axis so rotations count too.
fn distance(a: &[M], b: &[M]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let mut worst = 0f32;
            for probe in [[0., 0., 0.], [8., 0., 0.], [0., 8., 0.], [0., 0., 8.]] {
                let p = |m: &M| {
                    std::array::from_fn::<f32, 3, _>(|r| {
                        m[r][0] * probe[0] + m[r][1] * probe[1] + m[r][2] * probe[2] + m[r][3]
                    })
                };
                let (u, v) = (p(x), p(y));
                worst = worst.max(
                    ((u[0] - v[0]).powi(2) + (u[1] - v[1]).powi(2) + (u[2] - v[2]).powi(2)).sqrt(),
                );
            }
            worst
        })
        .fold(0., f32::max)
}

pub fn run(directory: &Path, output: &Path) -> Result<(), String> {
    let mut files = std::fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "djk_fnemote"))
        .collect::<Vec<_>>();
    files.sort();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let rows = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(12)
        {
            scope.spawn(|| loop {
                let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(file) = files.get(n) else { break };
                let result = (|| -> Result<Vec<Value>, String> {
                    let bundle = Bundle::open_index(file)?.validate_for_playback()?;
                    let pose = bundle.pose.clone().ok_or("bundle has no pose track")?;
                    let bones = pose.bones;
                    let frame = |f: usize| &pose.matrices()[f * bones..(f + 1) * bones];
                    let root = &bundle.manifest["emotes"][0];
                    let variants = root["variants"]
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| vec![root.clone()]);
                    let mut out = Vec::new();
                    for v in variants {
                        let (Some(first), Some(end)) =
                            (v["first_frame"].as_u64(), v["end_frame"].as_u64())
                        else {
                            continue;
                        };
                        let (first, end) = (first as usize, end as usize);
                        if end <= first + 1 || end > pose.frames {
                            continue;
                        }
                        let looping = v["loop"].as_bool().unwrap_or(false);
                        let start = v["loop_start_frame"]
                            .as_u64()
                            .map(|n| n as usize)
                            .unwrap_or(first);
                        let mut steps = (first..end - 1)
                            .map(|f| distance(frame(f), frame(f + 1)))
                            .collect::<Vec<_>>();
                        steps.sort_by(f32::total_cmp);
                        let p95 = steps
                            .get(steps.len() * 95 / 100)
                            .copied()
                            .unwrap_or(0.)
                            .max(0.05);
                        let seam = if looping {
                            distance(frame(end - 1), frame(start))
                        } else {
                            distance(frame(end - 1), frame(first))
                        };
                        let loop_seconds = (end - start) as f32 / 30.;
                        // Local continuity: the seam should move about as much as
                        // the frames on either side of it. ~0 means a repeated
                        // pose (hitch), >>1 a pop.
                        let target = if looping { start } else { first };
                        let before = distance(frame(end - 2), frame(end - 1));
                        let after = if target + 1 < end {
                            distance(frame(target), frame(target + 1))
                        } else {
                            before
                        };
                        let local = seam / ((before + after) * 0.5).max(0.05);
                        out.push(json!({
                            "id": file.file_stem().unwrap().to_string_lossy(),
                            "variant": v["variant_name"],
                            "loop": looping,
                            "frames": end - first,
                            "loop_start": start - first,
                            "loop_seconds": loop_seconds,
                            "seam": seam,
                            "p95_step": p95,
                            "ratio": seam / p95,
                            "local": local,
                            "before": before,
                            "after": after,
                        }));
                    }
                    Ok(out)
                })();
                match result {
                    Ok(r) => rows.lock().unwrap().extend(r),
                    Err(e) => rows.lock().unwrap().push(
                        json!({"id": file.file_stem().unwrap().to_string_lossy(), "error": e}),
                    ),
                }
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by(|a, b| {
        b["ratio"]
            .as_f64()
            .unwrap_or(0.)
            .total_cmp(&a["ratio"].as_f64().unwrap_or(0.))
    });
    let looping = rows
        .iter()
        .filter(|r| r["loop"] == true)
        .collect::<Vec<_>>();
    let pops = looping
        .iter()
        .filter(|r| r["ratio"].as_f64().unwrap_or(0.) > 3.)
        .count();
    let one_shot_cycles = rows
        .iter()
        .filter(|r| r["loop"] == false && r["ratio"].as_f64().unwrap_or(9.) < 1.)
        .count();
    let local_pops = looping
        .iter()
        .filter(|r| r["local"].as_f64().unwrap_or(0.) > 4. && r["seam"].as_f64().unwrap_or(0.) > 4.)
        .count();
    let stalls = looping
        .iter()
        .filter(|r| {
            r["local"].as_f64().unwrap_or(1.) < 0.25 && r["before"].as_f64().unwrap_or(0.) > 2.
        })
        .count();
    let summary = json!({"variants": rows.len(), "looping": looping.len(), "looping_seams_over_3x_motion": pops, "local_pops": local_pops, "repeated_pose_hitches": stalls, "one_shots_ending_on_first_pose": one_shot_cycles});
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({"summary": summary, "variants": rows})).unwrap(),
    )
    .map_err(|e| e.to_string())
}
