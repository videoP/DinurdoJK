use crate::ui::ColorLutPreset;
use oximedia_lut::creative_grade::FilmPreset;
use jka_assets::pk3::AssetSearchPath;
use std::path::Path;
use std::sync::RwLock;

pub const LUT_SIZE: u32 = 33;

/// Package-relative folder scanned for `.cube` files, so loose files under
/// `base/` (or the active mod dir) and files inside PK3s are found alike.
pub const LUT_QPATH_PREFIX: &str = "luts/";
const MAX_CUBE_SIZE: usize = 129;
const MAX_CUBE_BYTES: usize = 64 * 1024 * 1024;

struct ExternalLut {
    /// Upper-cased file stem for menus (the VFS only keeps lowercase names).
    label: &'static str,
    /// Lowercase file stem used as the `r_colorLut` value.
    key: &'static str,
    /// Lowercase package-relative path, e.g. `luts/bluehour.cube`.
    qpath: String,
}

static EXTERNAL: RwLock<Vec<ExternalLut>> = RwLock::new(Vec::new());

/// Rebuild the external LUT list from every `luts/*.cube` in the asset search
/// path (loose files and PK3s, mod directory above base).
pub fn scan_external(base: &Path, game: Option<&Path>) {
    let mut found: Vec<ExternalLut> = Vec::new();
    match AssetSearchPath::open_game(base, game) {
        Ok(assets) => {
            for name in assets.names() {
                let Some(file) = name.strip_prefix(LUT_QPATH_PREFIX) else {
                    continue;
                };
                let Some(stem) = file.strip_suffix(".cube") else {
                    continue;
                };
                if stem.is_empty() || stem.contains('/') {
                    continue;
                }
                found.push(ExternalLut {
                    label: Box::leak(stem.to_ascii_uppercase().into_boxed_str()),
                    key: Box::leak(stem.to_owned().into_boxed_str()),
                    qpath: name.to_owned(),
                });
            }
        }
        Err(error) => eprintln!("Color LUT asset search: {error}"),
    }
    found.sort_by(|a, b| a.key.cmp(b.key));
    if let Ok(mut list) = EXTERNAL.write() {
        *list = found;
    }
}

pub fn external_count() -> usize {
    EXTERNAL.read().map_or(0, |list| list.len())
}

pub fn external_label(index: u16) -> &'static str {
    EXTERNAL
        .read()
        .ok()
        .and_then(|list| list.get(index as usize).map(|lut| lut.label))
        .unwrap_or("MISSING LUT")
}

pub fn external_key(index: u16) -> &'static str {
    EXTERNAL
        .read()
        .ok()
        .and_then(|list| list.get(index as usize).map(|lut| lut.key))
        .unwrap_or("off")
}

pub fn find_external(name: &str) -> Option<u16> {
    let name = name.trim().to_ascii_lowercase();
    let name = name.strip_suffix(".cube").unwrap_or(&name);
    let list = EXTERNAL.read().ok()?;
    list.iter()
        .position(|lut| lut.key == name)
        .map(|index| index as u16)
}

fn external_qpath(index: u16) -> Option<String> {
    EXTERNAL
        .read()
        .ok()
        .and_then(|list| list.get(index as usize).map(|lut| lut.qpath.clone()))
}

/// Parse an Adobe/Resolve `.cube` 3D LUT and resample it to `LUT_SIZE`^3 RGBA8.
/// Inputs are assumed to be display-referred 0..1; `DOMAIN_*` lines are ignored.
pub fn parse_cube_rgba8(text: &str) -> Result<Vec<u8>, String> {
    let mut size = 0usize;
    let mut values: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let first = parts.next().unwrap_or("");
        match first {
            "TITLE" | "DOMAIN_MIN" | "DOMAIN_MAX" | "LUT_3D_INPUT_RANGE" => {}
            "LUT_1D_SIZE" => return Err("1D LUTs are not supported".to_owned()),
            "LUT_3D_SIZE" => {
                size = parts
                    .next()
                    .and_then(|value| value.parse().ok())
                    .ok_or("bad LUT_3D_SIZE")?;
                if !(2..=MAX_CUBE_SIZE).contains(&size) {
                    return Err(format!("unsupported LUT_3D_SIZE {size}"));
                }
                values.reserve(size * size * size);
            }
            _ => {
                let rgb: Vec<f32> = std::iter::once(first)
                    .chain(parts)
                    .filter_map(|value| value.parse().ok())
                    .collect();
                if rgb.len() != 3 {
                    return Err(format!("unrecognized line: {line}"));
                }
                values.push([rgb[0], rgb[1], rgb[2]]);
            }
        }
    }
    if size == 0 {
        return Err("missing LUT_3D_SIZE".to_owned());
    }
    if values.len() != size * size * size {
        return Err(format!(
            "expected {} entries, found {}",
            size * size * size,
            values.len()
        ));
    }

    let out_size = LUT_SIZE as usize;
    // .cube stores R fastest, then G, then B: the same order as our texture.
    let at = |r: usize, g: usize, b: usize| values[(b * size + g) * size + r];
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let mut data = Vec::with_capacity(out_size * out_size * out_size * 4);
    for b in 0..out_size {
        for g in 0..out_size {
            for r in 0..out_size {
                let rgb = if size == out_size {
                    at(r, g, b)
                } else {
                    let scale = (size - 1) as f32 / (out_size - 1) as f32;
                    let (fr, fg, fb) = (r as f32 * scale, g as f32 * scale, b as f32 * scale);
                    let (r0, g0, b0) = (fr as usize, fg as usize, fb as usize);
                    let (r1, g1, b1) = (
                        (r0 + 1).min(size - 1),
                        (g0 + 1).min(size - 1),
                        (b0 + 1).min(size - 1),
                    );
                    let (tr, tg, tb) = (fr - r0 as f32, fg - g0 as f32, fb - b0 as f32);
                    let mut out = [0.0f32; 3];
                    for (c, slot) in out.iter_mut().enumerate() {
                        let c00 = lerp(at(r0, g0, b0)[c], at(r1, g0, b0)[c], tr);
                        let c10 = lerp(at(r0, g1, b0)[c], at(r1, g1, b0)[c], tr);
                        let c01 = lerp(at(r0, g0, b1)[c], at(r1, g0, b1)[c], tr);
                        let c11 = lerp(at(r0, g1, b1)[c], at(r1, g1, b1)[c], tr);
                        *slot = lerp(lerp(c00, c10, tg), lerp(c01, c11, tg), tb);
                    }
                    out
                };
                for channel in rgb {
                    data.push((channel.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                }
                data.push(255);
            }
        }
    }
    Ok(data)
}

fn read_external(index: u16, base: &Path, game: Option<&Path>) -> Result<Vec<u8>, String> {
    let qpath = external_qpath(index).ok_or("LUT file no longer listed")?;
    let mut assets = AssetSearchPath::open_game(base, game).map_err(|error| error.to_string())?;
    let asset = assets
        .read(&qpath, MAX_CUBE_BYTES)
        .map_err(|error| format!("{qpath}: {error}"))?
        .ok_or_else(|| format!("{qpath}: not found"))?;
    let text = String::from_utf8_lossy(&asset.bytes);
    parse_cube_rgba8(&text).map_err(|error| format!("{qpath}: {error}"))
}

/// Build the 3D LUT texels for a preset, returning `(edge_size, rgba8)`.
pub fn build_rgba8(preset: ColorLutPreset, base: &Path, game: Option<&Path>) -> (u32, Vec<u8>) {
    match preset {
        ColorLutPreset::Off => (2, identity_rgba8(2)),
        ColorLutPreset::External(index) => match read_external(index, base, game) {
            Ok(data) => (LUT_SIZE, data),
            Err(error) => {
                eprintln!("Color LUT load failed: {error}");
                (2, identity_rgba8(2))
            }
        },
        _ => (LUT_SIZE, bake_rgba8(preset)),
    }
}

/// Bake the selected OxiMedia film transform into a compact 33^3 RGBA8 3D LUT.
/// This runs only when the user changes presets; per-frame cost is one filtered
/// 3D texture lookup in the existing post pass.
fn bake_rgba8(preset: ColorLutPreset) -> Vec<u8> {
    let film = match preset {
        ColorLutPreset::Off | ColorLutPreset::External(_) => return identity_rgba8(2),
        ColorLutPreset::KodakVision3_250d => FilmPreset::kodak_vision3_250d(),
        ColorLutPreset::KodakPortra400 => FilmPreset::kodak_portra_400(),
        ColorLutPreset::FujiEterna500 => FilmPreset::fuji_eterna_500(),
        ColorLutPreset::FujiVelvia50 => FilmPreset::fuji_velvia_50(),
    };

    let size = LUT_SIZE as usize;
    let denom = (size - 1) as f64;
    let mut data = Vec::with_capacity(size * size * size * 4);
    // WebGPU 3D textures are laid out with X fastest. Map X/Y/Z to R/G/B.
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let input = [r as f64 / denom, g as f64 / denom, b as f64 / denom];
                let output = film.apply(&input);
                for channel in output {
                    data.push((channel.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                }
                data.push(255);
            }
        }
    }
    data
}

pub fn identity_rgba8(size: u32) -> Vec<u8> {
    let size = size.max(2) as usize;
    let denom = (size - 1) as f32;
    let mut data = Vec::with_capacity(size * size * size * 4);
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                data.extend_from_slice(&[
                    ((r as f32 / denom) * 255.0 + 0.5) as u8,
                    ((g as f32 / denom) * 255.0 + 0.5) as u8,
                    ((b as f32 / denom) * 255.0 + 0.5) as u8,
                    255,
                ]);
            }
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_cube(size: usize) -> String {
        let mut text = format!("TITLE \"t\"\nLUT_3D_SIZE {size}\n");
        let d = (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    text += &format!("{} {} {}\n", r as f32 / d, g as f32 / d, b as f32 / d);
                }
            }
        }
        text
    }

    #[test]
    fn cube_identity_resamples_to_identity() {
        for size in [2usize, 17, 33, 65] {
            let data = parse_cube_rgba8(&identity_cube(size)).unwrap();
            let expected = identity_rgba8(LUT_SIZE);
            assert_eq!(data.len(), expected.len());
            let max_diff = data
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(max_diff <= 1, "size {size}: max diff {max_diff}");
        }
    }
}

#[cfg(test)]
mod vfs_tests {
    use super::*;

    #[test]
    fn shipped_luts_load_through_asset_search_path() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/base");
        if !base.join("LUTs").is_dir() {
            return;
        }
        scan_external(&base, None);
        let count = external_count();
        assert!(count > 0, "no LUTs found through the asset search path");
        for index in 0..count as u16 {
            let (size, data) = build_rgba8(ColorLutPreset::External(index), &base, None);
            assert_eq!(size, LUT_SIZE, "{}", external_label(index));
            assert_eq!(data.len(), (LUT_SIZE as usize).pow(3) * 4);
        }
        assert_eq!(
            ColorLutPreset::from_config("bluehour"),
            Some(ColorLutPreset::External(find_external("BlueHour.cube").unwrap()))
        );
    }
}
