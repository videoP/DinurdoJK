#![allow(dead_code)]
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static RUNAWAYS: AtomicUsize = AtomicUsize::new(0);
static RESET_DISTANCE_BITS: AtomicUsize = AtomicUsize::new(0);
macro_rules! devprintln {
    ($level:expr, $($arg:tt)*) => {{
        let message = format!($($arg)*);
        if message.contains("garment runaway") {
            crate::RUNAWAYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(distance) = message.split('(').nth(1).and_then(|s| s.split('u').next()).and_then(|s| s.parse::<f32>().ok()) {
                crate::RESET_DISTANCE_BITS.fetch_max(distance.to_bits() as usize, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }};
}
#[path = "../../crates/jka-client/src/cgame/cloth.rs"]
mod cloth;
#[path = "../../crates/jka-client/src/cgame/cloth_body.rs"]
mod cloth_body;
use cloth::{ClothConfig, ClothMotion, ClothSurfaceFrame, ClothSystem};
use jka_assets::{
    animation::{animation_index, load_humanoid_animations},
    ghoul2::{
        parse_gla, parse_glm, skin_glm_surface, Ghoul2Animator, GlmModel, Matrix3x4,
        BONE_ANIM_OVERRIDE, BONE_ANIM_OVERRIDE_LOOP,
    },
    pk3::AssetSearchPath,
    skin::parse_skin,
};
use serde_json::{json, Value};

fn frames(
    glm: &GlmModel,
    visible: &[usize],
    lod: usize,
    pose: &[Matrix3x4],
) -> Vec<ClothSurfaceFrame> {
    glm.lods[lod]
        .surfaces
        .iter()
        .filter_map(|surface| {
            let name = &glm.hierarchy[surface.surface_index].name;
            if !visible.contains(&surface.surface_index)
                || !ClothSystem::is_cloth_surface_name(name)
            {
                return None;
            }
            let skinned = skin_glm_surface(surface, pose).unwrap();
            let transforms = surface
                .vertices
                .iter()
                .map(|vertex| {
                    let mut transform = [[0.0; 4]; 3];
                    for weight in &vertex.weights {
                        let bone = &pose[surface.bone_references[weight.local_bone_index]];
                        for r in 0..3 {
                            for c in 0..4 {
                                transform[r][c] += bone[r][c] * weight.weight;
                            }
                        }
                    }
                    transform
                })
                .collect();
            Some(ClothSurfaceFrame {
                surface_index: surface.surface_index,
                surface_name: name.clone(),
                bind_positions: surface.vertices.iter().map(|v| v.position).collect(),
                posed_positions: skinned.vertices.iter().map(|v| v.position).collect(),
                posed_normals: skinned.vertices.iter().map(|v| v.normal).collect(),
                skin_transforms: transforms,
                triangles: surface.triangles.clone(),
            })
        })
        .collect()
}
fn snapshot(
    glm: &GlmModel,
    visible: &[usize],
    lod: usize,
    pose: &[Matrix3x4],
    frames: &[ClothSurfaceFrame],
    output: &HashMap<usize, cloth::ClothOutput>,
    caps: &[cloth::ClothCapsule],
    label: &str,
) -> Value {
    let body = glm.lods[lod].surfaces.iter().filter(|surface| visible.contains(&surface.surface_index)
        && !ClothSystem::is_cloth_surface_name(&glm.hierarchy[surface.surface_index].name))
        .map(|surface| {
            let skin=skin_glm_surface(surface,pose).unwrap();
            json!({"name":glm.hierarchy[surface.surface_index].name,
                "points":skin.vertices.iter().map(|v|v.position).collect::<Vec<_>>(),"triangles":surface.triangles})
        }).collect::<Vec<_>>();
    json!({"label":label,"body":body,
        "cloth":frames.iter().map(|frame|json!({
            "name":frame.surface_name,"authored":frame.posed_positions,
            "points":output[&frame.surface_index].positions,"triangles":frame.triangles
        })).collect::<Vec<_>>(),
        "capsules":caps.iter().map(|c|json!({"a":c.a,"b":c.b,"radius":c.radius,"basis":c.basis})).collect::<Vec<_>>()})
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut base = PathBuf::from("target/release/base");
    let mut out = PathBuf::from("target/cloth-replay/results");
    let mut quick = false;
    let mut assert_stable = false;
    let mut selected_hz: Option<u32> = None;
    let mut selected_gap: Option<f32> = None;
    let mut selected_scenario: Option<String> = None;
    let mut model_path = String::from("models/players/jawa_jazzy/model.glm");
    let mut skin_path = String::from("models/players/jawa_jazzy/model_jazzy_hood.skin");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--base" => base = PathBuf::from(args.next().ok_or("--base needs a path")?),
            "--out" => out = PathBuf::from(args.next().ok_or("--out needs a path")?),
            "--quick" => quick = true,
            "--assert-stable" => assert_stable = true,
            "--hz" => selected_hz = Some(args.next().ok_or("--hz needs a rate")?.parse()?),
            "--clearance" => {
                selected_gap = Some(args.next().ok_or("--clearance needs a gap")?.parse()?)
            }
            "--scenario" => selected_scenario = Some(args.next().ok_or("--scenario needs a name")?),
            "--model" => model_path = args.next().ok_or("--model needs a GLM qpath")?,
            "--skin" => skin_path = args.next().ok_or("--skin needs a skin qpath")?,
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    fs::create_dir_all(&out)?;
    let mut search = AssetSearchPath::open(&base)?;
    let model_asset = search
        .read(&model_path, 128 << 20)?
        .ok_or("model not found")?;
    let glm = parse_glm(&model_asset.bytes)?;
    let skin_asset = search.read(&skin_path, 1 << 20)?.ok_or("skin not found")?;
    let skin = parse_skin(&skin_asset.bytes)?;
    let visible = glm
        .hierarchy
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            skin.iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(&h.name))
                .filter(|entry| !entry.shader.is_empty() && entry.shader != "*off")
                .map(|_| i)
        })
        .collect::<Vec<_>>();
    let gla_path = format!("{}.gla", glm.anim_name.trim_end_matches(".gla"));
    let gla = parse_gla(
        &search
            .read(&gla_path, 128 << 20)?
            .ok_or("GLA not found")?
            .bytes,
    )?;
    let animations = load_humanoid_animations(&mut search)?;
    println!(
        "model={} source={} skin={} GLA={} visible={}",
        model_path,
        model_asset.source.display(),
        skin_path,
        gla_path,
        visible.len()
    );
    for &i in &visible {
        println!(
            "surface {}: {} ({})",
            i,
            glm.hierarchy[i].name,
            glm.lods[0]
                .surfaces
                .iter()
                .find(|s| s.surface_index == i)
                .map_or(0, |s| s.vertices.len())
        );
    }
    let templates = cloth_body::build_body_templates(&glm, &gla, &visible, 0);
    for (bone, capsule) in &templates {
        println!(
            "body {} radius {:.2}u a={:?} b={:?}",
            gla.skeleton[*bone].name, capsule.radius, capsule.a, capsule.b
        );
    }
    let mut csv=String::from("scenario,clearance,hz,render_ms,animation_influence,runaways,solver_errors,max_render_offset_u,max_reset_offset_u,mean_offset_u,proxy_penetrating_vertices,max_proxy_penetration_u,rejected_steps,max_secondary_offset_u,mean_secondary_offset_u,new_penetrating_vertices,max_new_penetration_u,wall_ms\n");
    let mut summaries = Vec::new();
    let mut snapshots = Vec::new();
    let mut timeline = Vec::new();
    let mut failures = 0;
    let mut total_rejections = 0;
    let clearances = if quick {
        vec![0.0, 0.25, 1.0, 4.0]
    } else {
        vec![0.0, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0]
    };
    let rates = if quick {
        vec![(60, 16)]
    } else {
        vec![(15, 16), (60, 16), (60, 2), (240, 16)]
    };
    let rates = selected_hz.map(|hz| vec![(hz, 16)]).unwrap_or(rates);
    let clearances = selected_gap.map(|gap| vec![gap]).unwrap_or(clearances);
    for (hz, render_ms) in rates {
        for scenario in ["idle", "run_stop_turn", "jump", "arm_intersection"] {
            if selected_scenario.as_ref().is_some_and(|s| s != scenario) {
                continue;
            }
            for &clearance in &clearances {
                RUNAWAYS.store(0, Ordering::Relaxed);
                RESET_DISTANCE_BITS.store(0, Ordering::Relaxed);
                let started = std::time::Instant::now();
                let mut last_capture = -125;
                let mut max_secondary = 0.0_f32;
                let mut total_secondary = 0.0_f64;
                let mut secondary_samples = 0;
                let mut new_penetrations = 0;
                let mut max_new_penetration = 0.0_f32;
                let mut rejected = 0;
                let mut system = ClothSystem::default();
                system.set_config(ClothConfig {
                    enabled: true,
                    hz,
                    max_substeps: 8,
                    body_collision: true,
                    body_clearance: clearance,
                    wind_velocity: [120.0, 40.0, 0.0],
                    ..ClothConfig::default()
                });
                let mut animator = Ghoul2Animator::new(&gla);
                let root = Ghoul2Animator::bone_index(&gla, "model_root").unwrap_or(0);
                let mut previous_anim = "";
                let mut errors = 0;
                let mut max_offset = 0.0_f32;
                let mut total_offset = 0.0_f64;
                let mut samples = 0_usize;
                let mut penetrations = 0_usize;
                let mut max_penetration = 0.0_f32;
                let mut worst = None;
                let duration = if quick { 3000 } else { 5000 };
                for time in (0..=duration).step_by(render_ms as usize) {
                    let (anim_name, phase_start) = match scenario {
                        "idle" => ("BOTH_STAND1", 0),
                        "run_stop_turn" => {
                            if time % 2000 < 1000 {
                                ("BOTH_RUN1", time / 2000 * 2000)
                            } else {
                                ("BOTH_STAND1", time / 2000 * 2000 + 1000)
                            }
                        }
                        "jump" => {
                            if time % 1500 < 400 {
                                ("BOTH_STAND1", time / 1500 * 1500)
                            } else if time % 1500 < 950 {
                                ("BOTH_JUMP1", time / 1500 * 1500 + 400)
                            } else {
                                ("BOTH_INAIR1", time / 1500 * 1500 + 950)
                            }
                        }
                        _ => ("BOTH_ATTACK1", 0),
                    };
                    let anim_name = if animation_index(anim_name)
                        .and_then(|i| animations.get(i as i32))
                        .is_some_and(|a| a.num_frames > 0)
                    {
                        anim_name
                    } else {
                        "BOTH_STAND1"
                    };
                    if previous_anim != anim_name {
                        let a = animations
                            .get(animation_index(anim_name).unwrap() as i32)
                            .unwrap();
                        animator.set_bone_anim_index(
                            &gla,
                            root,
                            a.first_frame as i32,
                            (a.first_frame + a.num_frames) as i32,
                            BONE_ANIM_OVERRIDE | BONE_ANIM_OVERRIDE_LOOP,
                            50.0 / a.frame_lerp.abs().max(1) as f32,
                            time,
                            None,
                            100,
                        )?;
                        previous_anim = anim_name;
                    }
                    animator.clear_bone_angle_overrides();
                    if scenario == "arm_intersection" {
                        let angle = 65.0 * (time as f32 * 0.004).sin();
                        for name in ["rhumerus", "lhumerus"] {
                            animator.set_bone_angles_postmult(
                                &gla,
                                name,
                                [0.0, angle, 0.0],
                                2,
                                6,
                                1,
                            )?;
                        }
                    }
                    let pose = animator.evaluate_pose_openjk_root(&gla, time)?;
                    let garment = frames(&glm, &visible, 0, &pose);
                    let caps = cloth_body::pose_body_capsules(&templates, &gla, &pose);
                    let prepared = caps.iter().map(cloth::BodyShape::new).collect::<Vec<_>>();
                    let seconds = time as f32 * 0.001;
                    let yaw = if scenario == "run_stop_turn" {
                        seconds * 2.5
                    } else {
                        0.0
                    };
                    let (sin, cos) = yaw.sin_cos();
                    let origin = if scenario == "run_stop_turn" {
                        let cycle = seconds / 2.0;
                        [
                            cycle.floor() * 250.0 + (seconds % 2.0).min(1.0) * 250.0,
                            0.0,
                            0.0,
                        ]
                    } else if scenario == "jump" {
                        [
                            0.0,
                            0.0,
                            ((seconds % 1.5 - 0.4) / 0.55 * std::f32::consts::PI)
                                .sin()
                                .max(0.0)
                                * 45.0,
                        ]
                    } else {
                        [0.0; 3]
                    };
                    let output = match system.simulate_garment(
                        0,
                        "jawa_jazzy",
                        0,
                        &garment,
                        &caps,
                        ClothMotion {
                            axis: [[cos, sin, 0.0], [-sin, cos, 0.0], [0.0, 0.0, 1.0]],
                            origin,
                        },
                        time,
                    ) {
                        Ok(output) => output,
                        Err(error) => {
                            errors += 1;
                            if errors == 1 {
                                println!("ERROR {scenario} clearance={clearance}: {error}");
                            }
                            continue;
                        }
                    };
                    if let Some(diag) = system.diagnostics(0) {
                        max_secondary = max_secondary.max(diag.max_secondary_offset_units);
                        total_secondary += diag.mean_secondary_offset_units as f64;
                        secondary_samples += 1;
                        rejected = diag.rejected_steps;
                    }
                    if hz == 60
                        && render_ms == 16
                        && clearance == 1.0
                        && scenario != "idle"
                        && time - last_capture >= 125
                    {
                        last_capture = time;
                        timeline.push(snapshot(
                            &glm,
                            &visible,
                            0,
                            &pose,
                            &garment,
                            &output,
                            &caps,
                            &format!("{scenario} gap={clearance} time={time}ms"),
                        ));
                    }
                    let mut frame_max = 0.0_f32;
                    for frame in &garment {
                        let Some(rendered) = output.get(&frame.surface_index) else {
                            continue;
                        };
                        for (&p, &authored) in rendered.positions.iter().zip(&frame.posed_positions)
                        {
                            let delta = glam::Vec3::from_array(p)
                                .distance(glam::Vec3::from_array(authored));
                            frame_max = frame_max.max(delta);
                            total_offset += delta as f64;
                            samples += 1;
                            let mut penetration = 0.0_f32;
                            let mut new_penetration = 0.0_f32;
                            for shape in &prepared {
                                let point = rapier_cloth_core::Vec3::from_array(p) * 0.0254;
                                let depth = shape.penetration(point) / 0.0254;
                                let authored_depth = shape.penetration(
                                    rapier_cloth_core::Vec3::from_array(authored) * 0.0254,
                                ) / 0.0254;
                                penetration = penetration.max(depth);
                                new_penetration = new_penetration.max(depth - authored_depth);
                            }
                            if penetration > 0.25 {
                                penetrations += 1;
                            }
                            max_penetration = max_penetration.max(penetration);
                            if new_penetration > 0.25 {
                                new_penetrations += 1;
                            }
                            max_new_penetration = max_new_penetration.max(new_penetration);
                        }
                    }
                    if frame_max > max_offset || worst.is_none() {
                        max_offset = frame_max;
                        worst=Some(snapshot(&glm,&visible,0,&pose,&garment,&output,&caps,
                    &format!("{scenario} clearance={clearance} Hz={hz} time={time}ms max={max_offset:.1}u anim={anim_name} elapsed={}ms",time-phase_start)));
                    }
                }
                let resets = RUNAWAYS.load(Ordering::Relaxed);
                let reset_distance =
                    f32::from_bits(RESET_DISTANCE_BITS.load(Ordering::Relaxed) as u32);
                let mean = total_offset / samples.max(1) as f64;
                let mean_secondary = total_secondary / secondary_samples.max(1) as f64;
                let wall_ms = started.elapsed().as_millis();
                if resets > 0 || errors > 0 || rejected > 0 || max_offset > 100.0 {
                    failures += 1;
                }
                total_rejections += rejected;
                println!("{scenario:16} gap={clearance:4} Hz={hz:3} dt={render_ms:2} resets={resets:3} errors={errors:3} max={max_offset:7.2}u resetmax={reset_distance:7.2}u mean={mean:6.2}u");
                csv.push_str(&format!("{scenario},{clearance},{hz},{render_ms},0.35,{resets},{errors},{max_offset},{reset_distance},{mean},{penetrations},{max_penetration},{rejected},{max_secondary},{mean_secondary},{new_penetrations},{max_new_penetration},{wall_ms}\n"));
                summaries.push(json!({"scenario":scenario,"clearance":clearance,"hz":hz,"render_ms":render_ms,"resets":resets,"errors":errors,"max_offset":max_offset,"max_reset":reset_distance,"mean_offset":mean,"penetrating":penetrations,"max_penetration":max_penetration,"rejected_steps":rejected,"max_secondary":max_secondary,"mean_secondary":mean_secondary,"new_penetrations":new_penetrations,"max_new_penetration":max_new_penetration,"wall_ms":wall_ms}));
                if let Some(worst) = worst {
                    snapshots.push(worst);
                }
            }
        }
    }
    fs::write(out.join("metrics.csv"), csv)?;
    fs::write(
        out.join("snapshots.json"),
        serde_json::to_string(&snapshots)?,
    )?;
    fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summaries)?,
    )?;
    let html =
        include_str!("viewer.html").replace("__SNAPSHOTS__", &serde_json::to_string(&snapshots)?);
    fs::write(out.join("viewer.html"), html)?;
    let timeline_html =
        include_str!("viewer.html").replace("__SNAPSHOTS__", &serde_json::to_string(&timeline)?);
    fs::write(out.join("timeline.html"), timeline_html)?;
    println!(
        "Wrote {}. Failing cases={failures}; rejected steps={total_rejections}",
        out.display()
    );
    if assert_stable && failures > 0 {
        return Err(format!("{failures} cloth stability regressions").into());
    }
    Ok(())
}
