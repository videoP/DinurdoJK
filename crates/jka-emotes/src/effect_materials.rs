//! Native shaders for baked particle materials.
//!
//! Particle materials are almost always unlit translucent/additive graphs
//! whose shape comes from a mask, flipbook or flare texture multiplied by the
//! particle color. The cooked graph is not available, so this port selects the
//! shape texture by parameter role and by content (`texture-classes.json` from
//! `texture-audit`): normal maps are never used, channel-packed masks are
//! split into one channel, and a candidate must fade out at its borders,
//! because tiling noise/erosion maps are UV-panned inside the graph and are not
//! the sprite's outline. Materials with no qualifying texture use a radial
//! falloff, which is what Niagara's default sprite materials compute.
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::Path,
};

const EXCLUDE: &[&str] = &[
    "normal",
    "noise",
    "distort",
    "wpo",
    "height",
    "reflection",
    "ramp",
    "gradientmap",
    "lut",
    "cube",
    "refraction",
    "depth",
    "vat",
    "position",
    "rotation",
    "tiling",
    "erosion",
    "panner",
];
const SHAPE: &[&str] = &[
    "subuv",
    "flipbook",
    "sprite",
    "particle",
    "flare",
    "mask",
    "alpha",
    "shape",
    "halo",
    "glow",
    "spark",
    "star",
    "smoke",
    "cloud",
    "ring",
    "basetexture",
    "base",
    "diffuse",
    "albedo",
    "color",
    "texture",
    "tex",
    "emissive",
];
/// Largest particle texture shipped; sprites cover a small part of the screen.
const MAX_SIZE: u32 = 512;
/// Mean edge signal below which an image counts as a self-contained shape.
const EDGE_FADE: f64 = 0.12;

/// How a chosen texture becomes the particle stage image.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Use RGB and alpha (alpha derived from luminance when opaque).
    Color(String),
    /// White image whose alpha is one channel of a packed mask (0..=2) or luminance (3).
    Mask(String, usize),
}

fn class_of<'a>(classes: &'a Value, id: &str) -> &'a Value {
    &classes[id]
}

/// Content check for one candidate texture. `surface` is a mesh-particle
/// material: its texture is UV-mapped over the mesh, so it need not fade at
/// its borders (tiling is fine), and packed masks use their strongest channel.
fn qualify(id: &str, classes: &Value, surface: bool) -> Option<Shape> {
    let c = class_of(classes, id);
    let edge = |i: usize| c["edge"][i].as_f64().unwrap_or(1.);
    if surface {
        return match c["class"].as_str().unwrap_or("color") {
            "normal" | "unreadable" => None,
            "packed" => (0..3)
                .filter(|&ch| c["mean"][ch].as_f64().unwrap_or(0.) > 0.02)
                .max_by(|&a, &b| {
                    c["mean"][a]
                        .as_f64()
                        .unwrap_or(0.)
                        .total_cmp(&c["mean"][b].as_f64().unwrap_or(0.))
                })
                .map(|ch| Shape::Mask(id.to_owned(), ch)),
            "grayscale" => Some(Shape::Mask(id.to_owned(), 3)),
            _ => Some(Shape::Color(id.to_owned())),
        };
    }
    match c["class"].as_str().unwrap_or("color") {
        "normal" | "unreadable" => None,
        "packed" => (0..3)
            .filter(|&ch| edge(ch) < EDGE_FADE && c["mean"][ch].as_f64().unwrap_or(0.) > 0.02)
            .min_by(|&a, &b| edge(a).total_cmp(&edge(b)))
            .map(|ch| Shape::Mask(id.to_owned(), ch)),
        // Multi-hue masks: the union of their channels is the shape; the
        // particle color (the authored tint) colours it.
        "grayscale" => (if c["alpha"].as_f64().unwrap_or(0.) > 0.05 {
            edge(3)
        } else {
            edge(4)
        } < EDGE_FADE)
            .then(|| Shape::Mask(id.to_owned(), 3)),
        _ => (if c["alpha"].as_f64().unwrap_or(0.) > 0.05 {
            edge(3)
        } else {
            edge(4)
        } < EDGE_FADE)
            .then(|| Shape::Color(id.to_owned())),
    }
}

/// Pick the shape texture of a particle material by parameter role and content.
pub fn shape_texture(
    material: &Value,
    native: &Value,
    textures: &Path,
    lookup: &HashMap<String, String>,
    classes: &Value,
    surface: bool,
) -> Option<Shape> {
    let exists = |id: &str| textures.join(format!("{id}.png")).is_file();
    let mut candidates: Vec<(usize, String)> = Vec::new();
    if let Some(id) = native["diffuse"].as_str().filter(|id| exists(id)) {
        candidates.push((0, id.to_owned()));
    }
    for (name, path) in material["parameters"]["textures"]
        .as_object()
        .into_iter()
        .flatten()
    {
        let lower = name.to_lowercase().replace([' ', '_'], "");
        if EXCLUDE.iter().any(|x| lower.contains(x)) {
            continue;
        }
        let Some(path) = path.as_str() else { continue };
        let leaf = path.rsplit('.').next().unwrap_or("").to_lowercase();
        if leaf.contains("normal")
            || leaf.contains("noise")
            || leaf.contains("blank")
            || leaf.ends_with("_n")
            || leaf.contains("white") && !lower.contains("mask")
        {
            continue;
        }
        let Some(rank) = SHAPE.iter().position(|k| lower.contains(k)) else {
            continue;
        };
        let Some(id) = lookup.get(&path.to_lowercase()).cloned() else {
            continue;
        };
        if exists(&id) {
            candidates.push((rank + 1, id));
        }
    }
    candidates.sort();
    candidates.dedup_by(|a, b| a.1 == b.1);
    candidates
        .iter()
        .find_map(|(_, id)| qualify(id, classes, surface))
}

/// Single authored tint color, folded into particle color by the baker.
pub fn tint(material: &Value) -> Option<[f32; 3]> {
    let colors = material["parameters"]["colors"].as_object()?;
    let named = [
        "color",
        "tint",
        "colortint",
        "tintcolor",
        "basecolor",
        "particlecolor",
        "emissivecolor",
    ];
    let mut found = colors
        .iter()
        .filter(|(k, _)| named.contains(&k.to_lowercase().replace([' ', '_'], "").as_str()));
    let (_, c) = found.next()?;
    if found.next().is_some() {
        return None;
    }
    let mut rgb = ["R", "G", "B"].map(|k| c[k].as_f64().unwrap_or(1.).max(0.) as f32);
    let peak = rgb.iter().cloned().fold(0., f32::max);
    if peak > 1. {
        rgb.iter_mut().for_each(|x| *x /= peak);
    }
    (peak > 0.).then_some(rgb)
}

/// Radial falloff; `exponent` < 1 gives a hard-edged ball, > 1 a soft glow.
fn disc(exponent: f32) -> Vec<u8> {
    let size = 64u32;
    let mut img = image::RgbaImage::new(size, size);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let dx = (x as f32 + 0.5) / size as f32 * 2. - 1.;
        let dy = (y as f32 + 0.5) / size as f32 * 2. - 1.;
        let d = (dx * dx + dy * dy).sqrt();
        let a = (1. - d).clamp(0., 1.).powf(exponent);
        *px = image::Rgba([255, 255, 255, (a * 255.) as u8]);
    }
    png(&img)
}

/// Soft annulus for procedural ring/shockwave materials.
fn ring() -> Vec<u8> {
    let size = 128u32;
    let mut img = image::RgbaImage::new(size, size);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let dx = (x as f32 + 0.5) / size as f32 * 2. - 1.;
        let dy = (y as f32 + 0.5) / size as f32 * 2. - 1.;
        let d = (dx * dx + dy * dy).sqrt();
        let a = (1. - ((d - 0.78) / 0.18).abs()).clamp(0., 1.).powf(1.5);
        *px = image::Rgba([255, 255, 255, (a * 255.) as u8]);
    }
    png(&img)
}

/// Lumpy smoke/dust puff: radial falloff modulated by deterministic value noise.
fn puff() -> Vec<u8> {
    let size = 128u32;
    let hash = |x: i32, y: i32| -> f32 {
        let mut h = (x as u32).wrapping_mul(374761393) ^ (y as u32).wrapping_mul(668265263);
        h = (h ^ (h >> 13)).wrapping_mul(1274126177);
        (h >> 8) as f32 / (1u32 << 24) as f32
    };
    let noise = |x: f32, y: f32| -> f32 {
        let (xi, yi) = (x.floor() as i32, y.floor() as i32);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let s = |t: f32| t * t * (3. - 2. * t);
        let a = hash(xi, yi) + (hash(xi + 1, yi) - hash(xi, yi)) * s(fx);
        let b = hash(xi, yi + 1) + (hash(xi + 1, yi + 1) - hash(xi, yi + 1)) * s(fx);
        a + (b - a) * s(fy)
    };
    let mut img = image::RgbaImage::new(size, size);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let u = (x as f32 + 0.5) / size as f32;
        let v = (y as f32 + 0.5) / size as f32;
        let d = ((u * 2. - 1.).powi(2) + (v * 2. - 1.).powi(2)).sqrt();
        let n = 0.55 * noise(u * 6., v * 6.)
            + 0.3 * noise(u * 13., v * 13.)
            + 0.15 * noise(u * 27., v * 27.);
        let a = ((1. - d).clamp(0., 1.).powf(0.9) * (0.45 + 0.9 * n)).clamp(0., 1.);
        let shade = (200. + 55. * n) as u8;
        *px = image::Rgba([shade, shade, shade, (a * 255.) as u8]);
    }
    png(&img)
}

/// Shape for a material without any usable texture, from its name.
fn procedural(asset: &str, hard: bool) -> &'static str {
    let leaf = asset.rsplit(['/', '.']).next().unwrap_or("").to_lowercase();
    if leaf.contains("ring") || leaf.contains("shockwave") || leaf.contains("wave") {
        "ring"
    } else if ["smoke", "dust", "cloud", "fog", "mist", "steam", "puff"]
        .iter()
        .any(|k| leaf.contains(k))
    {
        "puff"
    } else if hard {
        "hard"
    } else {
        "soft"
    }
}

fn png(img: &image::RgbaImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// Build the stage image: capped size, alpha from luminance when opaque, and
/// packed masks reduced to one white channel.
fn stage_image(bytes: &[u8], shape: &Shape, modulate: bool) -> Result<Vec<u8>, String> {
    let mut img = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    if img.width().max(img.height()) > MAX_SIZE {
        let scale = MAX_SIZE as f32 / img.width().max(img.height()) as f32;
        img = image::imageops::resize(
            &img,
            ((img.width() as f32 * scale) as u32).max(1),
            ((img.height() as f32 * scale) as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
    }
    let opaque = img.pixels().all(|p| p[3] == 255);
    for p in img.pixels_mut() {
        *p = match shape {
            Shape::Mask(_, ch) if *ch < 3 => image::Rgba([255, 255, 255, p[*ch]]),
            Shape::Mask(..) => image::Rgba([
                255,
                255,
                255,
                if opaque {
                    p[0].max(p[1]).max(p[2])
                } else {
                    p[3]
                },
            ]),
            Shape::Color(_) if opaque => image::Rgba([p[0], p[1], p[2], p[0].max(p[1]).max(p[2])]),
            Shape::Color(_) => *p,
        };
        // Darken (modulate) stages read coverage from RGB, not alpha.
        if modulate {
            let a = p[3];
            *p = image::Rgba([a, a, a, 255]);
        }
    }
    Ok(png(&img))
}

/// Write `shaders/fnemote_fx.shader` and textures under `output`.
pub fn write(
    export: &Path,
    lookup: &HashMap<String, String>,
    used: &BTreeSet<String>,
    mesh_used: &BTreeSet<String>,
    sprite_used: &BTreeSet<String>,
    output: &Path,
) -> Result<usize, String> {
    let read = |p: &Path| -> Result<Value, String> {
        serde_json::from_slice(&fs::read(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    };
    let materials = read(&export.join("materials.json"))?;
    let native = read(&export.join("native-materials-v2.json")).unwrap_or(Value::Null);
    let classes = read(&export.join("texture-classes.json"))
        .map_err(|e| format!("texture-classes.json (run texture-audit first): {e}"))?;
    let textures = export.join("Textures");
    let dir = output.join("models").join("fnemote").join("fx");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(output.join("shaders")).map_err(|e| e.to_string())?;
    fs::write(dir.join("soft.png"), disc(1.6)).map_err(|e| e.to_string())?;
    fs::write(dir.join("hard.png"), disc(0.35)).map_err(|e| e.to_string())?;
    fs::write(dir.join("ring.png"), ring()).map_err(|e| e.to_string())?;
    fs::write(dir.join("puff.png"), puff()).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("white.png"),
        png(&image::RgbaImage::from_pixel(4, 4, image::Rgba([255; 4]))),
    )
    .map_err(|e| e.to_string())?;
    // Coverage-in-RGB variants of the procedural shapes for modulate stages.
    for (name, bytes) in [
        ("soft", disc(1.6)),
        ("hard", disc(0.35)),
        ("ring", ring()),
        ("puff", puff()),
        (
            "white",
            png(&image::RgbaImage::from_pixel(4, 4, image::Rgba([255; 4]))),
        ),
    ] {
        let mut img = image::load_from_memory(&bytes)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        for p in img.pixels_mut() {
            *p = image::Rgba([p[3], p[3], p[3], 255]);
        }
        fs::write(dir.join(format!("{name}-d.png")), png(&img)).map_err(|e| e.to_string())?;
    }
    let mut shaders = String::from("// Generated by djk-fnemote: particle material ports.\n");
    let mut written = BTreeSet::new();
    for key in used {
        let material = &materials[key];
        let blend = material["blend"].as_str().unwrap_or("BLEND_Translucent");
        let modulate = blend == "BLEND_Modulate";
        // Mesh-only materials are surfaces; a material shared by sprites and
        // meshes keeps the sprite shape, which is what most of its uses need.
        let surface = mesh_used.contains(key) && !sprite_used.contains(key);
        let texture =
            match shape_texture(material, &native[key], &textures, lookup, &classes, surface) {
                Some(shape) => {
                    let (id, mut suffix) = match &shape {
                        Shape::Color(id) => (id.clone(), "c".to_owned()),
                        Shape::Mask(id, ch) => (id.clone(), format!("m{ch}")),
                    };
                    if modulate {
                        suffix += "d";
                    }
                    let name = format!("t_{id}_{suffix}.png");
                    if written.insert(name.clone()) {
                        let bytes = fs::read(textures.join(format!("{id}.png")))
                            .map_err(|e| e.to_string())?;
                        fs::write(dir.join(&name), stage_image(&bytes, &shape, modulate)?)
                            .map_err(|e| e.to_string())?;
                    }
                    format!("models/fnemote/fx/{name}")
                }
                None if surface && modulate => "models/fnemote/fx/white-d.png".to_owned(),
                None if surface => "models/fnemote/fx/white.png".to_owned(),
                None if modulate => format!(
                    "models/fnemote/fx/{}-d.png",
                    procedural(material["asset"].as_str().unwrap_or(""), false)
                ),
                None => {
                    // Sphere-mask materials expose Hardness; hard balls keep a crisp rim.
                    let hard = material["parameters"]["scalars"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .any(|(k, v)| {
                            k.eq_ignore_ascii_case("hardness") && v.as_f64().unwrap_or(0.) >= 0.6
                        });
                    format!(
                        "models/fnemote/fx/{}.png",
                        procedural(material["asset"].as_str().unwrap_or(""), hard)
                    )
                }
            };
        let state = match blend {
            "BLEND_Additive" => "blendFunc GL_SRC_ALPHA GL_ONE\n\t\tglow",
            "BLEND_Modulate" => "blendFunc GL_ZERO GL_ONE_MINUS_SRC_COLOR",
            "BLEND_Masked" | "BLEND_Opaque" if surface => "depthWrite",
            "BLEND_Masked" | "BLEND_Opaque" => "alphaFunc GE128\n\t\tdepthWrite",
            _ => "blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA",
        };
        shaders += &format!("fnemote/fx/m_{key}\n{{\n\tcull none\n\tnopicmip\n\t{{\n\t\tmap {texture}\n\t\t{state}\n\t\trgbGen vertex\n\t\talphaGen vertex\n\t}}\n}}\n");
    }
    fs::write(output.join("shaders").join("fnemote_fx.shader"), shaders)
        .map_err(|e| e.to_string())?;
    Ok(used.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn normal_maps_tiling_noise_and_packed_masks_are_handled() {
        let classes = json!({
            "n": {"class":"normal","edge":[0.5,0.5,1.0,1.0,1.0]},
            "tile": {"class":"grayscale","alpha":0.0,"edge":[0.6,0.6,0.6,1.0,0.6],"mean":[0.5,0.5,0.5,1.0]},
            "pack": {"class":"packed","alpha":0.0,"edge":[0.4,0.01,0.5,1.0,0.5],"mean":[0.3,0.2,0.4,1.0]},
            "flare": {"class":"color","alpha":0.0,"edge":[0.0,0.0,0.0,1.0,0.02],"mean":[0.3,0.2,0.1,1.0]}
        });
        assert_eq!(qualify("n", &classes, false), None);
        assert_eq!(qualify("tile", &classes, false), None);
        assert_eq!(
            qualify("pack", &classes, false),
            Some(Shape::Mask("pack".into(), 1))
        );
        assert_eq!(
            qualify("flare", &classes, false),
            Some(Shape::Color("flare".into()))
        );
    }
}
