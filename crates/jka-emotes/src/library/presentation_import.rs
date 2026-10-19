//! Bake source bone/socket transforms and montage notify lifetimes without
//! changing existing character poses or sound tracks.
use super::*;
use glam::{Mat3, Mat4, Quat, Vec3};
use std::{collections::HashMap, io::Write};

fn reference(v: &Value) -> Option<String> {
    let path = v["ObjectPath"].as_str()?;
    let name = v["ObjectName"]
        .as_str()?
        .split('\'')
        .nth(1)?
        .rsplit(':')
        .next()?;
    Some(format!("{}.{}", path.rsplit_once('.')?.0, name))
}
fn vec(v: &Value, default: Vec3) -> Vec3 {
    if v.is_null() {
        default
    } else if let Some(n) = v.as_f64() {
        Vec3::splat(n as f32)
    } else {
        Vec3::new(
            v["X"].as_f64().unwrap_or(default.x as f64) as f32,
            v["Y"].as_f64().unwrap_or(default.y as f64) as f32,
            v["Z"].as_f64().unwrap_or(default.z as f64) as f32,
        )
    }
}
fn rotation(v: &Value) -> Quat {
    let pitch = v["Pitch"].as_f64().unwrap_or(0.) as f32;
    let yaw = v["Yaw"].as_f64().unwrap_or(0.) as f32;
    let roll = v["Roll"].as_f64().unwrap_or(0.) as f32;
    // Unreal FRotator: yaw about Z, pitch about Y, roll about X. Reflect Y.
    let q = Quat::from_rotation_z(yaw.to_radians())
        * Quat::from_rotation_y(-pitch.to_radians())
        * Quat::from_rotation_x(-roll.to_radians());
    Quat::from_xyzw(-q.x, q.y, -q.z, q.w)
}
fn own_animation(
    directory: &Path,
    lookup: &HashMap<String, String>,
    props: &Value,
    mesh_asset: &str,
    def: &Value,
    start: f64,
    frames: usize,
) -> Result<Option<Vec<jka_assets::ghoul2::Matrix3x4>>, String> {
    let Some(asset) = reference(&props["SkeletalMeshPropAnimation"]) else {
        return Ok(None);
    };
    let key = lookup
        .get(&asset.to_lowercase())
        .ok_or("prop animation not exported")?;
    let mesh_key = lookup
        .get(&mesh_asset.to_lowercase())
        .ok_or("prop mesh not exported")?;
    let mesh: Value = serde_json::from_slice(&read(
        &directory
            .join("Meshes")
            .join(format!("{mesh_key}.skeletal.json")),
        MAX_MEMBER_BYTES,
    )?)
    .map_err(|e| e.to_string())?;
    let objects: Value = serde_json::from_slice(&read(
        &directory.join("Objects").join(format!("{key}.json")),
        MAX_MEMBER_BYTES,
    )?)
    .map_err(|e| e.to_string())?;
    let objects = objects
        .as_array()
        .ok_or("missing prop animation metadata")?;
    let name = asset.rsplit('.').next().unwrap_or("");
    let object = objects
        .iter()
        .find(|o| o["Name"] == name)
        .ok_or("prop animation object missing")?;
    let mut blueprint_slot = None;
    if let Some(blueprint) = reference(&props["SkeletalMeshPropAnimClass"]) {
        if let Some(id) = lookup.get(&blueprint.to_lowercase()) {
            let package: Value = serde_json::from_slice(&read(
                &directory.join("Objects").join(format!("{id}.json")),
                MAX_MEMBER_BYTES,
            )?)
            .map_err(|e| e.to_string())?;
            if let Ok(graph) = jka_ue_assets::blueprint::AnimationGraph::parse(&package) {
                blueprint_slot = Some(graph.slot(false)?.to_owned());
            }
        }
    }
    let psa = if object["Type"] == "AnimMontage" {
        let track = object["Properties"]["SlotAnimTracks"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .find(|t| {
                        blueprint_slot.as_ref().is_some_and(|name| {
                            t["SlotName"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(name))
                        })
                    })
                    .or_else(|| {
                        a.iter()
                            .find(|t| t["SlotName"] == "DefaultSlot" || t["SlotName"] == "FullBody")
                    })
                    .or_else(|| a.first())
            })
            .ok_or("no prop animation track")?;
        let mut segments = Vec::new();
        for s in track["AnimTrack"]["AnimSegments"]
            .as_array()
            .ok_or("no prop segments")?
        {
            let sequence = reference(&s["AnimReference"]).ok_or("no prop sequence")?;
            let id = lookup
                .get(&sequence.to_lowercase())
                .ok_or("prop sequence not exported")?;
            segments.push(json!({"psa":format!("Animations/{id}.psa"),"start":s["StartPos"],"from":s["AnimStartTime"],"to":s["AnimEndTime"],"rate":s["AnimPlayRate"],"repeats":s["LoopingCount"]}));
        }
        let duration = object["Properties"]["SequenceLength"]
            .as_f64()
            .ok_or("no prop montage length")?;
        montage::compose(
            directory,
            &json!({"segments":segments,"spans":[{"from":0,"to":duration,"at":0}],"duration_seconds":duration}),
        )?
    } else {
        crate::psa::Psa::parse(&read(
            &directory.join("Animations").join(format!("{key}.psa")),
            MAX_MEMBER_BYTES,
        )?)?
    };
    let mesh_rest = jka_ue_assets::transform::reference_matrices(&mesh["bones"])?;
    let bones = mesh["bones"]
        .as_array()
        .ok_or("missing independent prop skeleton")?
        .iter()
        .map(|b| {
            psa.bones
                .iter()
                .position(|p| {
                    b["Name"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(&p.name))
                })
                .ok_or("prop sequence/mesh bone mismatch")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::with_capacity(frames * bones.len());
    for frame in 0..frames {
        let span = def["spans"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|s| frame as f64 >= s["at"].as_f64().unwrap() * 30. - 1e-5)
            .ok_or("missing prop montage span")?;
        let time = span["from"].as_f64().unwrap() + frame as f64 / 30.
            - span["at"].as_f64().unwrap()
            - if props["bSyncMontage"].as_bool().unwrap_or(false) {
                0.
            } else {
                start
            };
        let source_frame = (time.max(0.) * psa.fps as f64).round() as usize;
        let source_frame = if props["bPropAnimLooping"].as_bool().unwrap_or(false) {
            source_frame % psa.frames
        } else {
            source_frame.min(psa.frames - 1)
        };
        let mut world = Vec::new();
        for (i, b) in psa.bones.iter().enumerate() {
            let k = source_frame * psa.bones.len() + i;
            let local = Mat4::from_scale_rotation_translation(
                psa.scales[k],
                psa.keys[k].rotation,
                psa.keys[k].position,
            );
            world.push(b.parent.map_or(local, |p| world[p] * local));
        }
        for (mesh_bone, &bone) in bones.iter().enumerate() {
            let mut m = world[bone] * mesh_rest[mesh_bone].inverse();
            m.w_axis *= 0.4;
            m.w_axis.w = 1.;
            let cols = m.to_cols_array_2d();
            out.push(std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r])));
        }
    }
    Ok(Some(out))
}
pub fn intervals(spans: &Value, start: f64, end: f64) -> Vec<[f32; 2]> {
    let mut out = Vec::<[f32; 2]>::new();
    for span in spans.as_array().into_iter().flatten() {
        let from = span["from"].as_f64().unwrap_or(0.);
        let to = span["to"].as_f64().unwrap_or(0.);
        let at = span["at"].as_f64().unwrap_or(0.);
        let a = start.max(from);
        let b = end.min(to);
        if b > a + 1e-6 {
            let next = [(at + a - from) as f32, (at + b - from) as f32];
            if let Some(last) = out
                .last_mut()
                .filter(|last| (last[1] - next[0]).abs() < 1e-4)
            {
                last[1] = next[1]
            } else {
                out.push(next)
            }
        }
    }
    out
}
pub fn run(
    source: &Path,
    metadata: &Path,
    authoring: &Path,
    shipping: &Path,
    assets: &Path,
    output: &Path,
) -> Result<(), String> {
    if output.exists() {
        return Err("presentation output already exists".into());
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let context = crate::effect_bake::NativeContext::load(assets)?;
    let asset_index = &context.index;
    let models = asset_index["models"]
        .as_array()
        .ok_or("missing native models")?
        .iter()
        .filter_map(|r| {
            Some((
                r["asset"].as_str()?.to_lowercase(),
                r["runtime_model"]
                    .as_str()
                    .or_else(|| r["model"].as_str())?
                    .to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();
    let unsupported_materials = asset_index["materials"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["status"] == "unsupported_graph" || m["status"] == "neutral_port")
        .filter_map(|m| m["id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    let model_gaps = asset_index["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| {
            let gaps = m["material_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|id| {
                    id.is_null()
                        || id
                            .as_str()
                            .is_some_and(|id| unsupported_materials.contains(id))
                })
                .count();
            (gaps > 0).then(|| (m["asset"].as_str().unwrap_or("").to_lowercase(), gaps))
        })
        .collect::<HashMap<_, _>>();
    let export_dir = &context.export_dir;
    let export_lookup = &context.export_lookup;
    let mut sockets = HashMap::<String, Value>::new();
    if let Some((_, key)) = export_lookup.iter().find(|(asset, _)| {
        asset.ends_with("fortnite_m_avg_player_skeleton.fortnite_m_avg_player_skeleton")
    }) {
        let objects: Value = serde_json::from_slice(&read(
            &export_dir.join("Objects").join(format!("{key}.json")),
            MAX_MEMBER_BYTES,
        )?)
        .map_err(|e| e.to_string())?;
        for o in objects
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| o["Type"] == "SkeletalMeshSocket")
        {
            if let Some(name) = o["Properties"]["SocketName"].as_str() {
                sockets.insert(name.to_lowercase(), o["Properties"].clone());
            }
        }
    }
    let systems = crate::effect_bake::SystemCache::default();
    let used_materials = std::sync::Mutex::new(std::collections::BTreeSet::new());
    let mesh_materials = std::sync::Mutex::new(std::collections::BTreeSet::new());
    let sprite_materials = std::sync::Mutex::new(std::collections::BTreeSet::new());
    let render_assets = context.render_assets(&used_materials, &mesh_materials, &sprite_materials);
    let mut files = fs::read_dir(shipping)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|r| r.path())
        .filter(|p| p.extension().is_some_and(|e| e == "djk_fnemote"))
        .collect::<Vec<_>>();
    files.sort();
    let progress = std::sync::atomic::AtomicUsize::new(0);
    let process = |file: &PathBuf| -> Option<Value> {
        let eid = file.file_stem().unwrap().to_string_lossy();
        let meta = metadata.join(format!("{eid}.json"));
        if !meta.is_file() {
            return None;
        }
        let data: Value = match read(&meta, MAX_MEMBER_BYTES)
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        {
            Ok(d) => d,
            Err(error) => return Some(json!({"id":eid,"error":error})),
        };
        let objects = data["Packages"]
            .as_object()
            .into_iter()
            .flat_map(|p| p.values())
            .flat_map(|a| a.as_array().into_iter().flatten())
            .collect::<Vec<_>>();
        let result = (|| {
            let shipped = Bundle::open_index(file)?;
            let mut root = shipped.manifest.clone();
            let author_file = authoring
                .to_string_lossy()
                .split(';')
                .map(|dir| Path::new(dir).join(file.file_name().unwrap()))
                .find(|p| p.is_file())
                .ok_or("authoring bundle missing")?;
            let author = Bundle::open_index(&author_file)?;
            let recipe: Value =
                serde_json::from_slice(&author.read("metadata/recipe.json", MAX_MEMBER_BYTES)?)
                    .map_err(|e| e.to_string())?;
            let definitions = recipe["animation"]["variants"]
                .as_array()
                .ok_or("no authoring variants")?;
            let conversion = author.manifest["conversion"]["variants"]
                .as_array()
                .ok_or("no source conversion provenance")?;
            let mut additions = BTreeMap::new();
            let mut count = 0;
            let mut missing = Vec::new();
            let default_name = root["emotes"][0]["variant_name"].clone();
            let rows = root["emotes"][0]["variants"]
                .as_array_mut()
                .ok_or("no shipping variants")?;
            for row in rows.iter_mut() {
                let missing_start = missing.len();
                let name = row["variant_name"].as_str().unwrap_or("");
                let Some(def) = definitions.iter().find(|v| v["name"] == name) else {
                    continue;
                };
                let Some(conv) = conversion
                    .iter()
                    .find(|v| v["variant_name"] == name)
                    .map(|v| &v["conversion"])
                else {
                    continue;
                };
                let Some(montage) = objects.iter().find(|o| {
                    o["Type"] == "AnimMontage" && o["Name"].as_str() == name.split(" / ").next()
                }) else {
                    continue;
                };
                let mut notify_rows = Vec::new();
                for notify in montage["Properties"]["Notifies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    let Some(obj_name) = notify["NotifyStateClass"]["ObjectName"]
                        .as_str()
                        .and_then(|s| s.split(':').next_back())
                        .map(|s| s.trim_end_matches('\''))
                    else {
                        continue;
                    };
                    let Some(obj) = objects.iter().find(|o| {
                        o["Name"] == obj_name
                            && o["Outer"]["ObjectName"]
                                == format!(
                                    "AnimMontage'{}'",
                                    montage["Name"].as_str().unwrap_or("")
                                )
                    }) else {
                        continue;
                    };
                    if !obj["Type"]
                        .as_str()
                        .is_some_and(|s| s.contains("SpawnProp"))
                    {
                        continue;
                    }
                    let props = &obj["Properties"];
                    let Some(asset) = reference(&props["StaticMeshProp"])
                        .or_else(|| reference(&props["SkeletalMeshProp"]))
                    else {
                        missing.push(format!("{}: spawn class has no resolved mesh", obj_name));
                        continue;
                    };
                    let Some(model) = models.get(&asset.to_lowercase()) else {
                        missing.push(format!("{asset}: native model missing"));
                        continue;
                    };
                    let start = notify["LinkValue"].as_f64().unwrap_or(0.);
                    let end = notify["EndLink"]["LinkValue"]
                        .as_f64()
                        .unwrap_or(start + notify["Duration"].as_f64().unwrap_or(0.));
                    let life = intervals(&def["spans"], start, end);
                    if !life.is_empty() {
                        notify_rows.push((props, model, asset, life, start));
                    }
                }
                let mut effect_specs = Vec::new();
                for notify in montage["Properties"]["Notifies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    let name = notify["NotifyStateClass"]["ObjectName"]
                        .as_str()
                        .or_else(|| notify["Notify"]["ObjectName"].as_str())
                        .unwrap_or("");
                    let obj_name = name
                        .split(':')
                        .next_back()
                        .unwrap_or("")
                        .trim_end_matches('\'');
                    let obj = objects.iter().find(|o| {
                        o["Name"] == obj_name
                            && o["Outer"]["ObjectName"]
                                == format!(
                                    "AnimMontage'{}'",
                                    montage["Name"].as_str().unwrap_or("")
                                )
                    });
                    let class = obj
                        .and_then(|o| o["Type"].as_str())
                        .unwrap_or(name.split('\'').next().unwrap_or(""));
                    let start = notify["LinkValue"].as_f64().unwrap_or(0.);
                    let timed = class.contains("NotifyState") || class.contains("Timed");
                    let end = if timed {
                        notify["EndLink"]["LinkValue"]
                            .as_f64()
                            .unwrap_or(start + notify["Duration"].as_f64().unwrap_or(0.))
                    } else {
                        start + 1e-3
                    };
                    if let Some(spec) = obj.and_then(|o| {
                        crate::effect_bake::EffectSpec::from_notify(
                            class,
                            &o["Properties"],
                            intervals(&def["spans"], start, end.max(start + 1e-3)),
                        )
                    }) {
                        if !spec.intervals.is_empty() {
                            effect_specs.push(spec);
                        }
                    } else if name.contains("Niagara")
                        || name.contains("ParticleEffect")
                        || name.contains("SpawnSparksProp")
                    {
                        missing.push(format!(
                            "{class}: legacy Cascade/Blueprint effect is not ported"
                        ));
                    }
                }
                let recipe_root = recipe["source_root"]
                    .as_str()
                    .map(PathBuf::from)
                    .filter(|p| p.is_dir())
                    .unwrap_or_else(|| source.join(eid.as_ref()));
                let psa = montage::compose(&recipe_root, def)?;
                if psa.frames
                    != (row["end_frame"].as_i64().ok_or("missing end frame")?
                        - row["first_frame"].as_i64().ok_or("missing first frame")?)
                        as usize
                {
                    return Err(format!(
                        "{name}: composed presentation timeline differs from shipping animation"
                    ));
                }
                let rest = psa.world_rotations(None);
                let q = conv["basis_xyzw"].as_array().ok_or("no source basis")?;
                let basis = Quat::from_xyzw(
                    q[0].as_f64().unwrap() as f32,
                    q[1].as_f64().unwrap() as f32,
                    q[2].as_f64().unwrap() as f32,
                    q[3].as_f64().unwrap() as f32,
                );
                let scale = conv["translation_scale"]
                    .as_f64()
                    .ok_or("no source scale")? as f32;
                if let Some(heights) =
                    crate::retarget::source_contact_heights(&psa, basis, scale, 0, psa.frames)
                {
                    let matrices = heights
                        .into_iter()
                        .map(|height| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., height]])
                        .collect::<Vec<_>>();
                    let track = format!("contacts/{:04}.pose", additions.len());
                    additions.insert(
                        track.clone(),
                        crate::PoseClip::pack_compact(&crate::PoseClip::encode(
                            psa.frames, 1, &matrices,
                        )?)?,
                    );
                    row["contact_track"] = json!(track);
                }
                if !effect_specs.is_empty() {
                    let env = crate::effect_bake::PsaEnvironment::new(&psa, &sockets);
                    let first = row["first_frame"].as_i64().unwrap_or(0);
                    let loop_start = row["loop_start_frame"]
                        .as_i64()
                        .map(|n| (n - first).max(0) as usize)
                        .or(row["loop"].as_bool().unwrap_or(false).then_some(0));
                    if let Some(baked) = crate::effect_bake::bake_variant(
                        &export_dir,
                        &export_lookup,
                        &systems,
                        &render_assets,
                        &effect_specs,
                        &env,
                        basis,
                        scale,
                        psa.frames,
                        loop_start,
                    ) {
                        for d in baked.diagnostics {
                            missing.push(format!("effect {d}"));
                        }
                        if !baked.track.is_empty() {
                            let existing = additions
                                .iter()
                                .find(|(name, bytes)| {
                                    name.starts_with("effects/") && **bytes == baked.track
                                })
                                .map(|(n, _)| n.clone());
                            let track = match existing {
                                Some(n) => n,
                                None => {
                                    let name = format!(
                                        "effects/{:04}.fxt",
                                        additions
                                            .keys()
                                            .filter(|k| k.starts_with("effects/"))
                                            .count()
                                    );
                                    additions.insert(name.clone(), baked.track);
                                    name
                                }
                            };
                            row["effects"] = json!({"track":track,"renderers":baked.renderers.iter().map(|r|r.to_json()).collect::<Vec<_>>()});
                        }
                    }
                }
                let mapping = conv["bone_map"].as_object().ok_or("no source bone map")?;
                let mut props_out = Vec::new();
                for (props, model, asset, life, start) in notify_rows {
                    let socket = props["SocketName"].as_str().unwrap_or("Root");
                    let socket_def = sockets.get(&socket.to_lowercase());
                    let bone_name = socket_def
                        .and_then(|s| s["BoneName"].as_str())
                        .unwrap_or(socket);
                    let Some(bone) = psa
                        .bones
                        .iter()
                        .position(|b| b.name.eq_ignore_ascii_case(bone_name))
                    else {
                        missing.push(format!("{asset}: socket {socket} absent from source PSA"));
                        continue;
                    };
                    let mut anchor = bone;
                    let target = loop {
                        if let Some((target, _)) = mapping.iter().find(|(_, s)| {
                            s.as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(&psa.bones[anchor].name))
                        }) {
                            break target.clone();
                        }
                        if let Some(parent) = psa.bones[anchor].parent {
                            anchor = parent
                        } else {
                            break "model_root".into();
                        }
                    };
                    let mapped_anchor = mapping
                        .get(&target)
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.eq_ignore_ascii_case(&psa.bones[anchor].name));
                    let offset = vec(&props["LocationOffset"], Vec3::ZERO);
                    let offset = Vec3::new(offset.x, -offset.y, offset.z);
                    let socket_rotation = socket_def
                        .map(|s| rotation(&s["RelativeRotation"]))
                        .unwrap_or(Quat::IDENTITY);
                    let socket_location = socket_def
                        .map(|s| vec(&s["RelativeLocation"], Vec3::ZERO))
                        .unwrap_or(Vec3::ZERO);
                    let socket_location =
                        Vec3::new(socket_location.x, -socket_location.y, socket_location.z);
                    let offset = socket_location + socket_rotation * offset;
                    let offset_rotation = socket_rotation * rotation(&props["RotationOffset"]);
                    let sizing = vec(
                        props
                            .get("Scale")
                            .or_else(|| props.get("Scale3D"))
                            .unwrap_or(&Value::Null),
                        Vec3::ONE,
                    );
                    let mut matrices = Vec::with_capacity(psa.frames);
                    for frame in 0..psa.frames {
                        let rotations = psa.world_rotations(Some(frame));
                        let mut positions = Vec::with_capacity(psa.bones.len());
                        for (i, b) in psa.bones.iter().enumerate() {
                            let p = psa.keys[frame * psa.bones.len() + i].position;
                            positions.push(b.parent.map_or(p, |j| positions[j] + rotations[j] * p));
                        }
                        // model_root is intentionally absent from the body bone
                        // map (controller owns translation). Its skin matrix
                        // therefore carries no source-root rotation. Preserve
                        // that rotation here instead of cancelling it as if the
                        // target anchor had already received it.
                        let anchor_rotation = if mapped_anchor {
                            rest[anchor] * rotations[anchor].inverse()
                        } else {
                            Quat::IDENTITY
                        };
                        let r = Mat3::from_quat(
                            basis * anchor_rotation * rotations[bone] * offset_rotation,
                        ) * Mat3::from_diagonal(sizing);
                        let t = basis
                            * anchor_rotation
                            * (positions[bone] + rotations[bone] * offset - positions[anchor])
                            * scale;
                        let cols = r.to_cols_array_2d();
                        matrices.push(std::array::from_fn(|i| {
                            [cols[0][i], cols[1][i], cols[2][i], t[i]]
                        }));
                    }
                    let track = format!("props/{count:04}.pose");
                    let mut skin_track = None;
                    if model.ends_with(".djk_prop") {
                        match own_animation(
                            &export_dir,
                            &export_lookup,
                            props,
                            &asset,
                            def,
                            start,
                            psa.frames,
                        ) {
                            Ok(Some(matrices)) => {
                                let bones = matrices.len() / psa.frames;
                                let name = format!("props/{count:04}.skin");
                                let packed =
                                    match crate::PoseClip::encode(psa.frames, bones, &matrices)
                                        .and_then(|bytes| crate::PoseClip::pack_compact(&bytes))
                                    {
                                        Ok(bytes) => bytes,
                                        Err(error) => {
                                            missing.push(format!(
                                                "{asset}: independent animation {error}"
                                            ));
                                            continue;
                                        }
                                    };
                                additions.insert(name.clone(), packed);
                                skin_track = Some(name);
                            }
                            Ok(None) => {}
                            Err(error) => {
                                missing.push(format!("{asset}: {error}"));
                                continue;
                            }
                        }
                    }
                    let packed = match crate::PoseClip::encode(psa.frames, 1, &matrices)
                        .and_then(|raw| crate::PoseClip::pack_compact(&raw))
                    {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            missing.push(format!("{asset}: attachment {error}"));
                            continue;
                        }
                    };
                    additions.insert(track.clone(), packed);
                    count += 1;
                    if !props["SkeletalMeshPropAnimClass"].is_null() {
                        missing.push(format!(
                            "{asset}: additional Animation Blueprint behavior is not ported"
                        ));
                    }
                    if !props["PropMaterialScalarParams"].is_null() {
                        missing.push(format!(
                            "{asset}: per-prop dynamic material parameters are not ported"
                        ));
                    }
                    if let Some(gaps) = model_gaps.get(&asset.to_lowercase()) {
                        missing.push(format!("{asset}: {gaps} material sections use neutral substitutes for procedural shader graphs"));
                    }
                    props_out.push(json!({"name":asset.rsplit('.').next().unwrap_or("Prop").trim_start_matches("SM_").trim_start_matches("SK_").trim_start_matches("Emote_Prop_").replace('_'," "),"model":model,"bone":target,"track":track,"skin_track":skin_track,"scale":scale/0.4,"intervals":life,"source_socket":socket,"source_asset":asset}));
                }
                row["props"] = json!(props_out);
                row["presentation_detail"] = json!(missing[missing_start..].join("\n"));
            }
            if additions.is_empty() && missing.is_empty() {
                return Ok(json!({"id":eid,"props":0,"missing":missing}));
            }
            if let Some(default) = rows
                .iter()
                .find(|v| v["variant_name"] == default_name)
                .cloned()
            {
                root["emotes"][0]["props"] = default["props"].clone();
                root["emotes"][0]["presentation_detail"] = default["presentation_detail"].clone();
                root["emotes"][0]["contact_track"] = default["contact_track"].clone();
                root["emotes"][0]["effects"] = default["effects"].clone();
            }
            let mut input = zip::ZipArchive::new(fs::File::open(file).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let dest = output.join(file.file_name().unwrap());
            let mut writer =
                zip::ZipWriter::new(fs::File::create(&dest).map_err(|e| e.to_string())?);
            writer
                .start_file(
                    "manifest.json",
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .map_err(|e| e.to_string())?;
            writer
                .write_all(&serde_json::to_vec_pretty(&root).unwrap())
                .map_err(|e| e.to_string())?;
            for i in 0..input.len() {
                let member = input.by_index(i).map_err(|e| e.to_string())?;
                if member.name() != "manifest.json"
                    && !member.name().starts_with("props/")
                    && !member.name().starts_with("contacts/")
                    && !member.name().starts_with("effects/")
                {
                    writer.raw_copy_file(member).map_err(|e| e.to_string())?;
                }
            }
            for (name, bytes) in additions {
                writer
                    .start_file(
                        name,
                        zip::write::SimpleFileOptions::default()
                            .compression_method(zip::CompressionMethod::Zstd)
                            .compression_level(Some(15)),
                    )
                    .map_err(|e| e.to_string())?;
                writer.write_all(&bytes).map_err(|e| e.to_string())?;
            }
            writer.finish().map_err(|e| e.to_string())?;
            Bundle::open_index(&dest)?;
            Ok::<_, String>(json!({"id":eid,"props":count,"missing":missing}))
        })();
        let done = progress.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if done % 50 == 0 {
            println!("Presentation {done}/{}", files.len());
        }
        Some(result.unwrap_or_else(|error| json!({"id":eid,"error":error})))
    };
    let next = std::sync::atomic::AtomicUsize::new(0);
    let collected = std::sync::Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 12);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(file) = files.get(n) else { break };
                if let Some(record) = process(file) {
                    collected.lock().unwrap().push((n, record));
                }
            });
        }
    });
    let mut collected = collected.into_inner().unwrap();
    collected.sort_by_key(|r| r.0);
    let records = collected.into_iter().map(|r| r.1).collect::<Vec<_>>();
    let used = used_materials.into_inner().unwrap();
    let mesh_used = mesh_materials.into_inner().unwrap();
    let sprite_used = sprite_materials.into_inner().unwrap();
    let shaders = crate::effect_materials::write(
        &export_dir,
        &export_lookup,
        &used,
        &mesh_used,
        &sprite_used,
        &output.join("fx-native"),
    )?;
    println!("Particle materials ported: {shaders}");
    let failures = records.iter().filter(|r| r.get("error").is_some()).count();
    fs::write(
        output.join("presentation-report.json"),
        serde_json::to_vec_pretty(&json!({"version":1,"items":records})).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    if failures > 0 {
        return Err(format!("{failures} presentation items failed; inspect presentation-report.json before packaging"));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notify_intersections_follow_reordered_sections() {
        let spans = json!([{"from":4,"to":6,"at":0},{"from":0,"to":2,"at":2}]);
        assert_eq!(intervals(&spans, 1., 5.), vec![[0., 1.], [3., 4.]]);
    }
    #[test]
    fn unreal_socket_yaw_is_reflected_with_the_mesh() {
        let q = rotation(&json!({"Yaw":90}));
        assert!((q * Vec3::X - Vec3::NEG_Y).length() < 1e-5);
    }
    #[test]
    fn scalar_scale_and_partial_vectors_keep_their_defaults() {
        assert_eq!(vec(&json!(0.5), Vec3::ONE), Vec3::splat(0.5));
        assert_eq!(vec(&json!({"X":2}), Vec3::ONE), Vec3::new(2., 1., 1.));
    }
}
