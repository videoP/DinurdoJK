//! Character idle effects (the `IdleEffectNiagara` of a UE character part,
//! e.g. Skull Trooper's eye trails) baked as looping particle tracks.
//!
//! A player animates live, so unlike emote effects these cannot be recorded
//! in model space along a known animation. Each system is simulated on the
//! character's static rest pose, attached to its socket, warmed up and then
//! recorded over a loop. Positions are relative to the socket's bone (the
//! bake's bone 0) in the usual Fortnite-to-JKA model axes, so the game moves
//! the loop with the skin matrix of the GLA bone that bone maps to.
//!
//!   bake-idle <native report.json> <request.json> <output dir>
//!
//! request: {"skeleton": [<mesh.skeletal.json>, ...], "sockets": [<SkeletalMeshSocket
//! properties>, ...], "effects": [{"system": "/Path/NS_X.NS_X", "socket": "head"}],
//! "warmup": 2, "loop": 4, "basis_xyzw": [..4], "translation_scale": 0.4}
//! (the last two default to the Fortnite player skeleton's values). Writes `idle<N>.fxt`, `idle.json` and the ported
//! particle materials under `fx-native/`.
use crate::effect_bake::{bake_variant, EffectSpec, NativeContext, PsaEnvironment, SystemCache};
use crate::effects::{Particle, Renderer, Track, Writer};
use glam::{Quat, Vec3};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

/// Defaults for the source skeleton to JKA model space transform (the values
/// the Fortnite player skeletons share, identical to every emote's
/// `conversion`). A request for another game's skeleton gives its own
/// `basis_xyzw` and `translation_scale`, as recorded in its emote conversions.
const DEFAULT_BASIS_XYZW: [f32; 4] = [0.00832, 0.0, 0.0, 0.99997];
const DEFAULT_TRANSLATION_SCALE: f32 = 0.40753;
const SIM_FPS: f32 = 30.;

/// Bones a particle may be re-anchored to (each maps to a `_humanoid` bone).
const ANCHOR_BONES: [&str; 6] = ["head", "hand_l", "hand_r", "spine_05", "pelvis", "root"];
/// A particle born further than this (game units) from every anchor keeps
/// the socket's bone.
const ANCHOR_REACH: f32 = 10.;

fn reflect(p: Vec3) -> Vec3 {
    Vec3::new(p.x, -p.y, p.z)
}

/// Split a baked track by the bone each particle is born nearest to, so
/// emitters that read skeleton sockets (sparkles on both hands, eyes) follow
/// those bones in game. A particle keeps its first bone for life, so playback
/// never interpolates one ID across two tracks. `anchors` holds (bone, offset
/// from the baked anchor) in the track's space. Returns (bone, renderers,
/// track) per bone used.
fn reanchor(
    track: &[u8],
    renderers: &[Renderer],
    baked: usize,
    anchors: &[(usize, Vec3)],
) -> Result<Vec<(usize, Vec<Renderer>, Vec<u8>)>, String> {
    let parsed = Track::parse(track)?;
    let baked_offset = anchors
        .iter()
        .find(|a| a.0 == baked)
        .map_or(Vec3::ZERO, |a| a.1);
    let nearest = |p: Vec3| {
        anchors
            .iter()
            .map(|&(b, offset)| (b, p.distance(offset)))
            .filter(|a| a.1 <= ANCHOR_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(baked, |a| a.0)
    };
    let offset_of = |bone: usize| {
        anchors
            .iter()
            .find(|a| a.0 == bone)
            .map_or(baked_offset, |a| a.1)
    };
    let mut born: HashMap<(u8, u16), usize> = HashMap::new();
    let mut frames: Vec<Vec<(usize, Particle)>> = Vec::with_capacity(parsed.frames());
    for f in 0..parsed.frames() {
        frames.push(
            parsed
                .frame(f)
                .iter()
                .map(|p| {
                    let bone = *born
                        .entry((p.renderer, p.id))
                        .or_insert_with(|| nearest(Vec3::from_array(p.position)));
                    (bone, *p)
                })
                .collect(),
        );
    }
    let mut bones: Vec<usize> = born.values().copied().collect();
    bones.sort_unstable();
    bones.dedup();
    let mut out = Vec::new();
    for bone in bones {
        let mut members: Vec<u8> = born
            .iter()
            .filter(|(_, &b)| b == bone)
            .map(|((r, _), _)| *r)
            .collect();
        members.sort_unstable();
        members.dedup();
        let remap: HashMap<u8, u8> = members
            .iter()
            .enumerate()
            .map(|(new, &old)| (old, new as u8))
            .collect();
        let offset = offset_of(bone);
        let mut writer = Writer::new(
            parsed.fps,
            members
                .iter()
                .map(|&r| renderers[r as usize].clone())
                .collect(),
        );
        for frame in &frames {
            let particles: Vec<Particle> = frame
                .iter()
                .filter(|(b, _)| *b == bone)
                .map(|(_, p)| Particle {
                    renderer: remap[&p.renderer],
                    position: (Vec3::from_array(p.position) - offset).to_array(),
                    ..*p
                })
                .collect();
            writer.push_frame(&particles);
        }
        if !writer.is_empty() {
            let group = writer.renderers.clone();
            out.push((bone, group, writer.finish()));
        }
    }
    Ok(out)
}

fn load(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// UE `FTransform` rotation and translation (the exporter's raw values).
fn ue_local(transform: &Value) -> (Vec3, Quat) {
    let v = |o: &Value, k: &str| o[k].as_f64().unwrap_or(0.) as f32;
    let t = &transform["Translation"];
    let r = &transform["Rotation"];
    let q = Quat::from_xyzw(
        v(r, "X"),
        v(r, "Y"),
        v(r, "Z"),
        if r["W"].is_null() { 1. } else { v(r, "W") },
    );
    (
        Vec3::new(v(t, "X"), v(t, "Y"), v(t, "Z")),
        if q.length_squared() > 1e-12 {
            q.normalize()
        } else {
            Quat::IDENTITY
        },
    )
}

/// Merged rest pose of the character parts (first part wins a shared bone),
/// as component-space (translation, rotation) in Unreal space.
fn rest_pose(meshes: &[Value]) -> Result<(Vec<String>, Vec<(Vec3, Quat)>), String> {
    let mut names: Vec<String> = Vec::new();
    let mut world: Vec<(Vec3, Quat)> = Vec::new();
    for mesh in meshes {
        let bones = mesh["bones"]
            .as_array()
            .ok_or("skeletal mesh has no bones")?;
        let mut part_world: Vec<(Vec3, Quat)> = Vec::with_capacity(bones.len());
        for bone in bones {
            let (t, q) = ue_local(&bone["Transform"]);
            let current = match bone["ParentIndex"]
                .as_i64()
                .filter(|&p| p >= 0)
                .map(|p| p as usize)
            {
                Some(p) => {
                    let &(pt, pq) = part_world.get(p).ok_or("bone stored before its parent")?;
                    (pt + pq * t, (pq * q).normalize())
                }
                None => (t, q),
            };
            part_world.push(current);
            let name = bone["Name"].as_str().unwrap_or("").to_owned();
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                names.push(name);
                world.push(current);
            }
        }
    }
    Ok((names, world))
}

pub fn run(report: &Path, request: &Path, output: &Path) -> Result<(), String> {
    let request = load(request)?;
    let meshes = request["skeleton"]
        .as_array()
        .ok_or("request has no skeleton meshes")?
        .iter()
        .map(|p| load(Path::new(p.as_str().unwrap_or(""))))
        .collect::<Result<Vec<_>, _>>()?;
    let (names, world) = rest_pose(&meshes)?;
    // Skeletal meshes face +Y in component space; the emote bake frame (PSA
    // animations, see `basis_xyzw`) faces +X. Yaw the rest pose to match.
    let yaw = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
    let world: Vec<(Vec3, Quat)> = world
        .into_iter()
        .map(|(t, q)| (yaw * t, (yaw * q).normalize()))
        .collect();
    let sockets: HashMap<String, Value> = request["sockets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| Some((s["SocketName"].as_str()?.to_lowercase(), s.clone())))
        .collect();
    let warmup = request["warmup"].as_f64().unwrap_or(2.) as f32;
    let loop_seconds = request["loop"].as_f64().unwrap_or(4.) as f32;
    let context = NativeContext::load(report)?;
    let (used, mesh_used, sprite_used) = Default::default();
    let render_assets = context.render_assets(&used, &mesh_used, &sprite_used);
    let systems = SystemCache::default();
    std::fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let basis_xyzw = match request["basis_xyzw"].as_array() {
        Some(a) if a.len() == 4 => {
            let v: Vec<f32> = a
                .iter()
                .filter_map(|n| n.as_f64())
                .map(|n| n as f32)
                .collect();
            <[f32; 4]>::try_from(v).map_err(|_| "basis_xyzw must be four numbers")?
        }
        Some(_) => return Err("basis_xyzw must be four numbers".into()),
        None => DEFAULT_BASIS_XYZW,
    };
    let basis = Quat::from_array(basis_xyzw).normalize();
    if !basis.is_finite() {
        return Err("basis_xyzw must be a finite, non-zero quaternion".into());
    }
    let translation_scale = request["translation_scale"]
        .as_f64()
        .map_or(DEFAULT_TRANSLATION_SCALE, |n| n as f32);
    if !translation_scale.is_finite() || translation_scale <= 0. {
        return Err("translation_scale must be positive".into());
    }
    let mut baked_effects = Vec::new();
    for (index, effect) in request["effects"]
        .as_array()
        .ok_or("request has no effects")?
        .iter()
        .enumerate()
    {
        let system = effect["system"].as_str().ok_or("effect has no system")?;
        let socket = effect["socket"].as_str().unwrap_or("root");
        // The game moves the loop with a bone, so bone 0 is the socket's bone
        // and recorded positions are relative to it; the system itself still
        // spawns at the socket.
        let bone = sockets
            .get(&socket.to_lowercase())
            .and_then(|s| s["BoneName"].as_str())
            .unwrap_or(socket)
            .to_owned();
        let Some(anchor) = names.iter().position(|n| n.eq_ignore_ascii_case(&bone)) else {
            println!("{system}: socket {socket} (bone {bone}) is not on the character; skipped");
            continue;
        };
        let mut env_names = vec!["__anchor".to_owned()];
        env_names.extend(names.iter().cloned());
        let mut env_world = vec![world[anchor]];
        env_world.extend(world.iter().copied());
        let env = PsaEnvironment::static_pose(env_names, env_world, &sockets);
        let total = warmup + loop_seconds;
        let spec = EffectSpec {
            template: system.to_owned(),
            socket: socket.to_owned(),
            location: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            attached: true,
            timed: true,
            destroy_at_end: false,
            // Never stopped: the loop region repeats while the model is shown.
            intervals: vec![[0., total + 3600.]],
        };
        let frames = (total * SIM_FPS).round() as usize;
        let Some(result) = bake_variant(
            &context.export_dir,
            &context.export_lookup,
            &systems,
            &render_assets,
            std::slice::from_ref(&spec),
            &env,
            basis,
            translation_scale,
            frames,
            Some((warmup * SIM_FPS).round() as usize),
        ) else {
            // Say why: missing system, no emitters, or emitters that never drew.
            match systems.get(&context.export_dir, &context.export_lookup, system) {
                None => println!("{system}: Niagara system not exported or not parsed"),
                Some(asset) => {
                    println!(
                        "{system}: nothing drawn in {total}s ({} emitters)",
                        asset.emitters.len()
                    );
                    for e in &asset.emitters {
                        println!(
                            "  {} gpu={} approx={} renderers={} spawn={:?} error={:?}",
                            e.name,
                            e.gpu,
                            e.approx.is_some(),
                            e.renderers.iter().filter(|r| r.enabled).count(),
                            e.spawn_infos,
                            e.error
                        );
                    }
                }
            }
            continue;
        };
        for d in &result.diagnostics {
            println!("{system}: {d}");
        }
        if result.track.is_empty() {
            println!("{system}: no particles recorded");
            continue;
        }
        // Emitters that read skeleton sockets (hands, eyes) follow those
        // bones in game, so each renderer moves to the nearest anchor bone.
        let to_model =
            |b: usize| basis * (reflect(world[b].0) - reflect(world[anchor].0)) * translation_scale;
        let anchors: Vec<(usize, Vec3)> = ANCHOR_BONES
            .iter()
            .filter_map(|name| names.iter().position(|n| n.eq_ignore_ascii_case(name)))
            .map(|b| (b, to_model(b)))
            .collect();
        for (part, (bone_index, renderers, bytes)) in
            reanchor(&result.track, &result.renderers, anchor, &anchors)?
                .into_iter()
                .enumerate()
        {
            let track = format!("idle{index}_{part}.fxt");
            std::fs::write(output.join(&track), &bytes).map_err(|e| e.to_string())?;
            println!(
                "{system}: baked {} renderers on {}",
                renderers.len(),
                names[bone_index]
            );
            baked_effects.push(json!({
                "system": system,
                "socket": socket,
                "bone": names[bone_index],
                "track": track,
                "loop_start": warmup,
                "renderers": renderers.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
            }));
        }
    }
    let shaders = crate::effect_materials::write(
        &context.export_dir,
        &context.export_lookup,
        &used.into_inner().unwrap(),
        &mesh_used.into_inner().unwrap(),
        &sprite_used.into_inner().unwrap(),
        &output.join("fx-native"),
    )?;
    std::fs::write(
        output.join("idle.json"),
        serde_json::to_vec_pretty(&json!({"version": 1, "effects": baked_effects})).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "Idle effects: {} baked, {shaders} particle materials",
        baked_effects.len()
    );
    Ok(())
}
