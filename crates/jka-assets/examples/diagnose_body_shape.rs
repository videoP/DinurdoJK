//! Capture the client's height deformation over the retail walking clip.
//! Usage: diagnose_body_shape <retail-base> <output.json> [model] [clip]
use jka_assets::{
    animation::{animation_index, parse_animation_cfg, HUMANOID_ANIMATION_CFG},
    body_shape::{anny, BodyShapeBinding, BodyShapeParams},
    ghoul2::*,
};
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if !(3..=5).contains(&args.len()) {
        return Err(
            "expected <retail-base> <output.json> [model=jedi_hf] [clip=BOTH_WALK1]".into(),
        );
    }
    let model = args.get(3).map(String::as_str).unwrap_or("jedi_hf");
    let clip = args.get(4).map(String::as_str).unwrap_or("BOTH_WALK1");
    // Read only the retail packages, independently of optional mod packages.
    let mut archives = (0..4)
        .map(|i| {
            Ok(zip::ZipArchive::new(std::fs::File::open(
                std::path::Path::new(&args[1]).join(format!("assets{i}.pk3")),
            )?)?)
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let mut read = |name: &str| -> Vec<u8> {
        for archive in archives.iter_mut().rev() {
            if let Ok(mut entry) = archive.by_name(name) {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                return bytes;
            }
        }
        panic!("missing {name}");
    };
    let original = parse_glm_native(&read(&format!("models/players/{model}/model.glm")))?;
    let gla = parse_gla(&read(&format!("{}.gla", original.anim_name)))?;
    let animations = parse_animation_cfg(&read(HUMANOID_ANIMATION_CFG))?;
    let animation = animations.animations[animation_index(clip).ok_or("unknown animation")?];
    if animation.num_frames == 0 {
        return Err("animation has no frames".into());
    }
    let skin = String::from_utf8(read(&format!("models/players/{model}/model_default.skin")))?;
    let off: Vec<_> = skin
        .lines()
        .filter_map(|l| l.split_once(','))
        .filter(|(_, v)| v.trim() == "*off")
        .map(|(n, _)| n.trim())
        .collect();
    let visible: Vec<_> = original.lods[0]
        .surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let h = &original.hierarchy[s.surface_index];
            !h.name.starts_with('*') && h.flags & 3 == 0 && !off.contains(&h.name.as_str())
        })
        .map(|(i, _)| i)
        .collect();
    let mut offset = 0;
    let mut triangles = Vec::new();
    for &si in &visible {
        let s = &original.lods[0].surfaces[si];
        triangles.extend(s.triangles.iter().map(|t| t.map(|i| i as usize + offset)));
        offset += s.vertices.len();
    }
    let mut cases = Vec::new();
    let neutral = BodyShapeParams::default();
    for (label, params) in [
        (
            "height_low",
            BodyShapeParams {
                height: 0.,
                ..neutral
            },
        ),
        ("neutral", neutral),
        (
            "height_high",
            BodyShapeParams {
                height: 1.,
                ..neutral
            },
        ),
        (
            "proportions_low",
            BodyShapeParams {
                proportions: 0.,
                ..neutral
            },
        ),
        (
            "proportions_high",
            BodyShapeParams {
                proportions: 1.,
                ..neutral
            },
        ),
        ("age_low", BodyShapeParams { age: 0., ..neutral }),
        ("age_high", BodyShapeParams { age: 1., ..neutral }),
    ] {
        let mut glm = original.clone();
        let pivots = anny::apply_profile(&mut glm, &gla, params, Default::default(), None)?;
        let rest: Vec<_> = pivots.clone().unwrap_or_else(|| {
            gla.skeleton
                .iter()
                .map(|b| [b.base_pose[0][3], b.base_pose[1][3], b.base_pose[2][3]])
                .collect()
        });
        let binding = pivots
            .as_ref()
            .map(|p| BodyShapeBinding::new(&gla, None, p))
            .transpose()?;
        let mut frames = Vec::new();
        let mut gaps = Vec::new();
        let left = gla
            .skeleton
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case("ltalus"))
            .ok_or("missing left ankle")?;
        let right = gla
            .skeleton
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case("rtalus"))
            .ok_or("missing right ankle")?;
        let start = usize::from(animation.first_frame);
        for frame in start..start + usize::from(animation.num_frames) {
            let mut animator = Ghoul2Animator::new(&gla);
            animator.set_bone_anim(
                &gla,
                "model_root",
                0,
                (gla.num_frames - 1) as i32,
                BONE_ANIM_OVERRIDE_FREEZE,
                1.,
                0,
                Some(frame as f32),
                0,
            )?;
            let stock = animator.evaluate_pose_openjk_root(&gla, 0)?;
            let pose = binding
                .as_ref()
                .map(|b| b.retarget(&gla, &stock))
                .transpose()?
                .unwrap_or_else(|| stock.clone());
            let joints: Vec<[f32; 3]> = pose
                .iter()
                .zip(&rest)
                .map(|(m, p)| {
                    std::array::from_fn(|k| m[k][3] + (0..3).map(|i| m[k][i] * p[i]).sum::<f32>())
                })
                .collect();
            // OpenJK's root turn maps bind-space lateral X onto pose-space Y.
            gaps.push(joints[left][1] - joints[right][1]);
            let mut vertices = Vec::new();
            for &si in &visible {
                vertices.extend(
                    skin_glm_surface(&glm.lods[0].surfaces[si], &pose)?
                        .vertices
                        .iter()
                        .map(|v| v.position),
                );
            }
            frames.push(serde_json::json!({"frame":frame,"joints":joints,"vertices":vertices,"stock":stock}));
        }
        let crossed = gaps.iter().filter(|&&g| g < 0.).count();
        let min_gap = gaps.iter().copied().fold(f32::INFINITY, f32::min);
        let max_gap = gaps.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        eprintln!("{model}/{clip}/{label}: crossed {crossed}/{} frames; ankle separation {min_gap:.4}..{max_gap:.4}", frames.len());
        cases.push(serde_json::json!({"label":label,"height":params.height,"proportions":params.proportions,"age":params.age,"rest":rest,"frames":frames,"crossed_frames":crossed,"min_ankle_separation":min_gap,"max_ankle_separation":max_gap}));
    }
    let bones: Vec<_> = gla.skeleton.iter().map(|b| &b.name).collect();
    std::fs::write(
        &args[2],
        serde_json::to_vec(
            &serde_json::json!({"model":model,"clip":clip,"bones":bones,"triangles":triangles,"cases":cases}),
        )?,
    )?;
    Ok(())
}
