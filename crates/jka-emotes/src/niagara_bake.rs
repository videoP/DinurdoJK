//! Execute exported Niagara systems with the native CPU host.
use jka_ue_runtime::niagara_sim::{
    NoSkeleton, SystemAsset, SystemInstance, TickContext, Transform,
};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

fn load(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| e.to_string())
}

/// Camera used for camera-relative modules: 3 m in front of the actor at
/// eye height, looking back at it (Unreal +X forward).
pub fn emote_camera() -> Transform {
    Transform {
        translation: glam::Vec3::new(300., 0., 120.),
        rotation: glam::Quat::from_rotation_z(std::f32::consts::PI),
        scale: glam::Vec3::ONE,
    }
}

/// Run every exported NiagaraSystem for `seconds` and report execution.
pub fn audit(
    directory: &Path,
    output: &Path,
    seconds: f32,
    filter: Option<&str>,
) -> Result<(), String> {
    let report = load(&directory.join("report.json"))?;
    let rows = report["assets"]
        .as_array()
        .ok_or("export report missing assets")?
        .iter()
        .filter(|r| r["type"] == "NiagaraSystem" && r["status"] != "failed")
        .filter(|r| {
            filter.is_none_or(|f| {
                r["asset"]
                    .as_str()
                    .is_some_and(|a| a.to_lowercase().contains(&f.to_lowercase()))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let lookup = report["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some((
                r["asset"].as_str()?.to_lowercase(),
                r["id"].as_str()?.to_owned(),
            ))
        })
        .collect();
    let fields = crate::effect_bake::FieldStore::new(directory, lookup);
    let next = AtomicUsize::new(0);
    let records = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let n = next.fetch_add(1, Ordering::Relaxed);
                let Some(row) = rows.get(n) else { break };
                let record = run_one(directory, row, seconds, &fields);
                records.lock().unwrap().push(record);
            });
        }
    });
    let mut records = records.into_inner().unwrap();
    records.sort_by(|a, b| a["asset"].as_str().cmp(&b["asset"].as_str()));
    let ok = records.iter().filter(|r| r["status"] == "ok").count();
    let visible = records
        .iter()
        .filter(|r| r["max_particles"].as_u64().unwrap_or(0) > 0)
        .count();
    let mut unbound = std::collections::BTreeMap::<String, usize>::new();
    let mut errors = std::collections::BTreeMap::<String, usize>::new();
    for r in &records {
        for u in r["unbound"].as_array().into_iter().flatten() {
            let key = u
                .as_str()
                .unwrap_or("")
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_owned();
            *unbound.entry(key).or_default() += 1;
        }
        for e in r["errors"].as_array().into_iter().flatten() {
            let s = e.as_str().unwrap_or("");
            let key = s
                .split_once(": ")
                .map_or(s, |x| x.1)
                .chars()
                .take(90)
                .collect::<String>();
            *errors.entry(key).or_default() += 1;
        }
    }
    let summary = json!({"systems":records.len(),"executed_without_error":ok,"produced_particles":visible,"unbound_functions":unbound,"error_kinds":errors});
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({"version":1,"summary":summary,"systems":records}))
            .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    Ok(())
}

fn run_one(
    directory: &Path,
    row: &Value,
    seconds: f32,
    fields: &crate::effect_bake::FieldStore,
) -> Value {
    let asset = row["asset"].clone();
    let id = row["id"].as_str().unwrap_or("");
    let package = match load(&directory.join("Objects").join(format!("{id}.json"))) {
        Ok(p) => p,
        Err(e) => return json!({"asset":asset,"status":"load_failed","errors":[e]}),
    };
    let parsed = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        SystemAsset::parse_with(&package, &|p| fields.get(p))
    })) {
        Ok(Ok(s)) => Arc::new(s),
        Ok(Err(e)) => return json!({"asset":asset,"status":"parse_failed","errors":[e]}),
        Err(_) => return json!({"asset":asset,"status":"parse_panicked"}),
    };
    let unbound = parsed.unbound_functions();
    let emitter_errors = parsed
        .emitters
        .iter()
        .filter_map(|e| e.error.as_ref().map(|x| format!("{}: {x}", e.name)))
        .collect::<Vec<_>>();
    let mut instance = SystemInstance::new(parsed.clone(), 1);
    let dt = 1. / 30.;
    let frames = (seconds / dt).ceil() as usize;
    let mut max = 0;
    let mut history = Vec::new();
    let env = NoSkeleton;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for frame in 0..frames {
            let ctx = TickContext {
                dt,
                owner: Transform::default(),
                owner_velocity: glam::Vec3::ZERO,
                time: frame as f32 * dt,
                env: &env,
                camera: emote_camera(),
            };
            instance.tick(&ctx);
            let n = instance.particle_count();
            max = max.max(n);
            if frame % 15 == 14 {
                history.push(n);
            }
            if instance.complete {
                break;
            }
        }
    }));
    let mut errors = emitter_errors;
    errors.extend(instance.errors.iter().cloned());
    if result.is_err() {
        errors.push("simulation panicked".into());
    }
    let mut bounds = Vec::new();
    for (e, em) in parsed.emitters.iter().enumerate() {
        for (r, _) in em.renderers.iter().enumerate() {
            let p = instance.particles(e, r);
            if let Some(first) = p.first() {
                let (mut lo, mut hi) = (first.position, first.position);
                for q in &p {
                    lo = lo.min(q.position);
                    hi = hi.max(q.position);
                }
                bounds.push(json!({"emitter":em.name,"renderer":r,"count":p.len(),"min":lo.to_array(),"max":hi.to_array(),"color0":first.color,"size0":first.size}));
                break;
            }
        }
    }
    json!({"asset":asset,"status":if errors.is_empty(){"ok"}else{"errors"},"emitters":parsed.emitters.len(),"gpu_emitters":parsed.emitters.iter().filter(|e|e.gpu).count(),"max_particles":max,"history":history,"complete":instance.complete,"errors":errors,"unbound":unbound,"bounds":bounds})
}

/// Contact sheet of one system: front view (Unreal Y right, Z up), 8 frames.
pub fn preview(directory: &Path, filter: &str, output: &Path, seconds: f32) -> Result<(), String> {
    let report = load(&directory.join("report.json"))?;
    let row = report["assets"]
        .as_array()
        .ok_or("export report missing assets")?
        .iter()
        .find(|r| {
            r["type"] == "NiagaraSystem"
                && r["asset"]
                    .as_str()
                    .is_some_and(|a| a.to_lowercase().contains(&filter.to_lowercase()))
        })
        .ok_or("no NiagaraSystem matches the filter")?;
    let package = load(
        &directory
            .join("Objects")
            .join(format!("{}.json", row["id"].as_str().unwrap_or(""))),
    )?;
    let lookup = report["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some((
                r["asset"].as_str()?.to_lowercase(),
                r["id"].as_str()?.to_owned(),
            ))
        })
        .collect();
    let fields = crate::effect_bake::FieldStore::new(directory, lookup);
    let asset = Arc::new(SystemAsset::parse_with(&package, &|p| fields.get(p))?);
    let mut instance = SystemInstance::new(asset.clone(), 1);
    let (cell, cols, rows) = (320u32, 4u32, 2u32);
    let mut image = image::RgbImage::new(cell * cols, cell * rows);
    let dt = 1. / 30.;
    let frames = (seconds / dt).ceil() as usize;
    let shots = (cols * rows) as usize;
    let env = NoSkeleton;
    let mut shot = 0;
    // 1 pixel = 1 cm around the actor, feet at the bottom quarter.
    let scale = cell as f32 / 400.;
    for frame in 0..frames {
        let ctx = TickContext {
            dt,
            owner: Transform::default(),
            owner_velocity: glam::Vec3::ZERO,
            time: frame as f32 * dt,
            env: &env,
            camera: emote_camera(),
        };
        instance.tick(&ctx);
        if shot < shots && frame + 1 >= (shot + 1) * frames / shots {
            let (ox, oy) = ((shot as u32 % cols) * cell, (shot as u32 / cols) * cell);
            for (e, em) in asset.emitters.iter().enumerate() {
                for (r, rd) in em.renderers.iter().enumerate() {
                    if !rd.enabled {
                        continue;
                    }
                    for p in instance.particles(e, r) {
                        let cx = cell as f32 / 2. + p.position.y * scale;
                        let cy = cell as f32 * 0.8 - p.position.z * scale;
                        let size = match rd.kind {
                            jka_ue_runtime::niagara_sim::RendererKind::Mesh => {
                                6. * p.scale.max_element()
                            }
                            jka_ue_runtime::niagara_sim::RendererKind::Ribbon => p.width,
                            jka_ue_runtime::niagara_sim::RendererKind::Light => 4.,
                            _ => p.size[0].max(p.size[1]),
                        };
                        let radius = (0.5 * size * scale).clamp(1., 60.);
                        let a = p.color[3].clamp(0., 1.);
                        let rgb = [p.color[0], p.color[1], p.color[2]]
                            .map(|c| (c.max(0.) / (1. + c.max(0.))) * 2.);
                        let (x0, x1) = ((cx - radius).floor() as i32, (cx + radius).ceil() as i32);
                        let (y0, y1) = ((cy - radius).floor() as i32, (cy + radius).ceil() as i32);
                        for y in y0.max(0)..y1.min(cell as i32) {
                            for x in x0.max(0)..x1.min(cell as i32) {
                                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt()
                                    / radius;
                                if d > 1. {
                                    continue;
                                }
                                let w = a * (1. - d) * 0.6;
                                let px = image.get_pixel_mut(ox + x as u32, oy + y as u32);
                                for c in 0..3 {
                                    px[c] =
                                        (px[c] as f32 + 255. * rgb[c].min(1.) * w).min(255.) as u8;
                                }
                            }
                        }
                    }
                }
            }
            // Floor line and frame border.
            for x in 0..cell {
                let y = (cell as f32 * 0.8) as u32;
                image.put_pixel(ox + x, oy + y, image::Rgb([60, 60, 90]));
                image.put_pixel(ox + x, oy, image::Rgb([90, 90, 90]));
            }
            shot += 1;
        }
    }
    image.save(output).map_err(|e| e.to_string())?;
    println!(
        "{}: {} emitters, errors {:?}",
        asset.name,
        asset.emitters.len(),
        instance.errors
    );
    Ok(())
}
