//! Offline QA render of a compiled bundle's baked effects over its pose.
//! Mirrors the client's placement: particles are relative to model_root's
//! skin matrix plus its bind translation, sprites face a front camera.
use crate::{
    effects::{Kind, Track},
    Bundle,
};
use jka_assets::ghoul2::{multiply_3x4, parse_gla, Matrix3x4};
use std::{collections::HashMap, path::Path};

struct Material {
    image: image::RgbaImage,
    additive: bool,
}

fn materials(fx_native: &Path) -> Result<HashMap<String, Material>, String> {
    let text = std::fs::read_to_string(fx_native.join("shaders/fnemote_fx.shader"))
        .map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    let mut name = String::new();
    let mut map = String::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with("fnemote/fx/") {
            name = line.to_owned();
        } else if let Some(m) = line.strip_prefix("map ") {
            map = m.to_owned();
        } else if line.starts_with("blendFunc") || line.starts_with("alphaFunc") {
            let image = image::open(fx_native.join(&map))
                .map_err(|e| format!("{map}: {e}"))?
                .to_rgba8();
            out.insert(
                name.clone(),
                Material {
                    image,
                    additive: line.contains("GL_ONE") && !line.contains("MINUS"),
                },
            );
        }
    }
    Ok(out)
}

pub fn run(bundle: &Path, variant: usize, fx_native: &Path, output: &Path) -> Result<(), String> {
    let bundle = Bundle::open_index(bundle)?.validate_for_playback()?;
    let emotes = crate::manifest::parse_manifest(&bundle.manifest)?;
    let root = emotes.first().ok_or("bundle has no emote")?;
    let emote = if root.variants.is_empty() {
        root.clone()
    } else {
        root.variants
            .get(variant)
            .cloned()
            .ok_or("variant out of range")?
    };
    let effects = emote
        .effects
        .as_ref()
        .ok_or("variant has no baked effects")?;
    let track = Track::parse(&bundle.read(&effects.track, 64 << 20)?)?;
    let gla = parse_gla(&bundle.read("animation.gla", 64 << 20)?)?;
    let pose = bundle.pose.clone().ok_or("bundle has no pose")?;
    let mats = materials(fx_native)?;
    let model_root = gla
        .skeleton
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case("model_root"))
        .unwrap_or(0);
    let (cell, cols, rows) = (360u32, 4u32, 2u32);
    let mut img =
        image::RgbaImage::from_pixel(cell * cols, cell * rows, image::Rgba([18, 18, 26, 255]));
    let frames = (emote.end_frame - emote.first_frame) as usize;
    let loop_start = emote
        .loop_start_frame
        .map(|n| (n - emote.first_frame) as usize)
        .or(emote.looping.then_some(0));
    let track_loop = loop_start.map(|l| (l as f32 / emote.fps * track.fps).round() as usize);
    let duration = frames as f32 / emote.fps;
    let scale = cell as f32 / 120.;
    let mut particles = Vec::new();
    for shot in 0..cols * rows {
        let seconds = duration * (shot as f32 + 0.5) / (cols * rows) as f32;
        let elapsed = (seconds * 1000.) as i32;
        let skins: Vec<Matrix3x4> = pose.sample_range_interpolated(
            elapsed,
            emote.fps,
            emote.first_frame as usize,
            emote.end_frame as usize,
            loop_start.map(|l| l + emote.first_frame as usize),
            crate::Interpolation::Linear,
        );
        let (ox, oy) = ((shot % cols) * cell, (shot / cols) * cell);
        // Front camera: screen x = +Y, screen y = -Z, feet near the bottom.
        let project = |p: [f32; 3]| {
            (
                ox as f32 + cell as f32 * 0.5 + p[1] * scale,
                oy as f32 + cell as f32 * 0.62 - p[2] * scale,
            )
        };
        for (b, bone) in gla.skeleton.iter().enumerate() {
            let world = multiply_3x4(&skins[b], &bone.base_pose);
            let (x, y) = project([world[0][3], world[1][3], world[2][3]]);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (px, py) = (x as i32 + dx, y as i32 + dy);
                    if px >= ox as i32
                        && py >= oy as i32
                        && px < (ox + cell) as i32
                        && py < (oy + cell) as i32
                    {
                        img.put_pixel(px as u32, py as u32, image::Rgba([120, 140, 170, 255]));
                    }
                }
            }
        }
        let skin = skins[model_root];
        let bind = gla.skeleton[model_root].base_pose;
        track.sample(seconds, track_loop, &mut particles);
        for p in &particles {
            let r = &effects.renderers[p.renderer as usize];
            let q = [
                p.position[0] + bind[0][3],
                p.position[1] + bind[1][3],
                p.position[2] + bind[2][3],
            ];
            let w: [f32; 3] = std::array::from_fn(|i| {
                skin[i][0] * q[0] + skin[i][1] * q[1] + skin[i][2] * q[2] + skin[i][3]
            });
            let (cx, cy) = project(w);
            let tint = p.rgba.map(|c| c as f32 / 255.);
            let (half_w, half_h, mat) = match r.kind {
                Kind::Sprite => (
                    p.size[0] * 0.5 * scale,
                    p.size[1] * 0.5 * scale,
                    mats.get(&r.shader),
                ),
                Kind::Ribbon => (
                    p.size[0] * 0.5 * scale,
                    p.size[0] * 0.5 * scale,
                    mats.get(&r.shader),
                ),
                Kind::Mesh => (
                    p.scale[0].abs().max(0.2) * 4. * scale,
                    p.scale[2].abs().max(0.2) * 4. * scale,
                    None,
                ),
                Kind::Light => continue,
            };
            let (cols_uv, rows_uv) = (r.sub_image[0].max(1) as u32, r.sub_image[1].max(1) as u32);
            let cell_uv = p.sub_image as u32 % (cols_uv * rows_uv);
            let (u0, v0) = (
                (cell_uv % cols_uv) as f32 / cols_uv as f32,
                (cell_uv / cols_uv) as f32 / rows_uv as f32,
            );
            let (hw, hh) = (
                half_w.clamp(0.5, cell as f32),
                half_h.clamp(0.5, cell as f32),
            );
            for y in (cy - hh).floor() as i32..=(cy + hh).ceil() as i32 {
                for x in (cx - hw).floor() as i32..=(cx + hw).ceil() as i32 {
                    if x < ox as i32
                        || y < oy as i32
                        || x >= (ox + cell) as i32
                        || y >= (oy + cell) as i32
                    {
                        continue;
                    }
                    let (s, t) = (
                        ((x as f32 - cx) / (2. * hw) + 0.5).clamp(0., 0.999),
                        ((y as f32 - cy) / (2. * hh) + 0.5).clamp(0., 0.999),
                    );
                    let (texel, additive) = match mat {
                        Some(m) => {
                            let tx = ((u0 + s / cols_uv as f32) * m.image.width() as f32) as u32;
                            let ty = ((v0 + t / rows_uv as f32) * m.image.height() as f32) as u32;
                            let c = m.image.get_pixel(
                                tx.min(m.image.width() - 1),
                                ty.min(m.image.height() - 1),
                            );
                            (
                                [c[0], c[1], c[2], c[3]].map(|v| v as f32 / 255.),
                                m.additive,
                            )
                        }
                        None => ([0.8, 0.8, 0.85, 1.], false),
                    };
                    let a = texel[3] * tint[3];
                    let px = img.get_pixel_mut(x as u32, y as u32);
                    for c in 0..3 {
                        let src = texel[c] * tint[c] * 255.;
                        let dst = px[c] as f32;
                        px[c] = if additive {
                            (dst + src * a).min(255.)
                        } else {
                            dst * (1. - a) + src * a
                        } as u8;
                    }
                }
            }
        }
        for x in 0..cell {
            img.put_pixel(ox + x, oy, image::Rgba([70, 70, 80, 255]));
            img.put_pixel(ox, oy + x.min(cell - 1), image::Rgba([70, 70, 80, 255]));
        }
    }
    img.save(output).map_err(|e| e.to_string())?;
    println!(
        "{} [{}]: {} renderers, {} track samples",
        emote.id,
        emote.variant_name,
        effects.renderers.len(),
        track.frames()
    );
    Ok(())
}
