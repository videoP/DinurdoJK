use crate::ui::ColorLutPreset;
use oximedia_lut::creative_grade::FilmPreset;

pub const LUT_SIZE: u32 = 33;

/// Bake the selected OxiMedia film transform into a compact 33^3 RGBA8 3D LUT.
/// This runs only when the user changes presets; per-frame cost is one filtered
/// 3D texture lookup in the existing post pass.
pub fn bake_rgba8(preset: ColorLutPreset) -> Vec<u8> {
    let film = match preset {
        ColorLutPreset::Off => return identity_rgba8(2),
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
