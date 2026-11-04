//! Lightmaps.
use crate::scene::{materials, AssetSearchPath, BTreeSet, TextureData};

pub(in crate::scene) fn external_lightmap_pages(mesh: &jka_assets::bsp::Mesh) -> BTreeSet<usize> {
    mesh.batches
        .iter()
        .flat_map(|batch| {
            (0..4).filter_map(move |slot| {
                (batch.lightmaps[slot] >= 0 && batch.lightmap_styles[slot] < 254)
                    .then_some(batch.lightmaps[slot] as usize)
            })
        })
        .collect()
}

pub(in crate::scene) fn embedded_deluxe_mapping(
    mesh: &jka_assets::bsp::Mesh,
    page_count: usize,
) -> bool {
    if page_count < 2 {
        return false;
    }
    let pages = external_lightmap_pages(mesh);
    !pages.is_empty()
        && pages
            .iter()
            .all(|&page| page % 2 == 0 && page + 1 < page_count)
}

pub(in crate::scene) fn hdr_lightmap_indexed(assets: &AssetSearchPath, map_name: &str) -> bool {
    let prefix = format!("maps/{map_name}/lm_").to_ascii_lowercase();
    assets.names().any(|name| {
        let lower = name.to_ascii_lowercase();
        lower.starts_with(&prefix) && lower.ends_with(".hdr")
    })
}

pub(in crate::scene) fn try_load_hdr_lightmap(
    assets: &mut AssetSearchPath,
    map_name: &str,
    page: usize,
) -> Result<Option<TextureData>, String> {
    let path = format!("maps/{map_name}/lm_{page:04}.hdr");
    let Some(asset) = assets
        .read(&path, 64 * 1024 * 1024)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let mut image =
        materials::decode_hdr_texture_data(&path, &asset.bytes, true, std::f32::consts::FRAC_1_PI)?;
    image.source = Some(asset.source);
    Ok(Some(image))
}

pub(in crate::scene) fn load_external_lightmap(
    assets: &mut AssetSearchPath,
    map_name: &str,
    page: usize,
    prefer_hdr: bool,
    map_overbright_bits: u8,
    output_overbright_bits: u8,
) -> Result<Option<TextureData>, String> {
    if prefer_hdr {
        if let Some(image) = try_load_hdr_lightmap(assets, map_name, page)? {
            return Ok(Some(image));
        }
    }
    let stem = format!("maps/{map_name}/lm_{page:04}");
    let candidates = [
        format!("{stem}.tga"),
        format!("{stem}.jpg"),
        format!("{stem}.png"),
    ];

    for path in &candidates {
        if let Some(asset) = assets
            .read(path, 64 * 1024 * 1024)
            .map_err(|error| error.to_string())?
        {
            let mut image = materials::decode_texture_data(path, &asset.bytes, true, false)?;
            image.source = Some(asset.source);
            shift_lightmap_rgba(&mut image, map_overbright_bits, output_overbright_bits);
            if prefer_hdr {
                materials::promote_lightmap_to_float(&mut image);
            }
            return Ok(Some(image));
        }
    }

    // TaystJK's R_LoadLightmaps leaves a missing external page unloaded;
    // R_FindLightmaps then chooses LIGHTMAP_BY_VERTEX for its surfaces.
    // Missing is not a corrupt image: preserve decoding/I/O errors above.
    Ok(None)
}

pub(in crate::scene) fn neutral_deluxemap(label: impl Into<String>) -> TextureData {
    TextureData {
        label: label.into(),
        source: None,
        width: 1,
        height: 1,
        rgba: (vec![127, 127, 127, 0]).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: false,
    }
}

pub(in crate::scene) fn normalize_deluxemap(mut image: TextureData) -> TextureData {
    image.srgb = false;
    image.rgba16f = None;
    for pixel in image.rgba.chunks_exact_mut(4) {
        if pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 {
            pixel[0] = 127;
            pixel[1] = 127;
            pixel[2] = 127;
        }
        // Alpha is reserved as a renderer-side validity marker. Authored
        // q3map2 deluxemaps do not need to carry meaningful alpha.
        pixel[3] = 255;
    }
    image
}

pub(in crate::scene) fn try_load_external_deluxemap(
    assets: &mut AssetSearchPath,
    map_name: &str,
    page: usize,
    paired_lightmaps: bool,
) -> Result<Option<TextureData>, String> {
    let mut stems = Vec::new();
    if paired_lightmaps {
        stems.push(format!("maps/{map_name}/lm_{:04}", page + 1));
        stems.push(format!("maps/{map_name}/dm_{:04}", page / 2));
    }
    stems.push(format!("maps/{map_name}/dm_{page:04}"));
    stems.dedup();
    for stem in stems {
        for ext in ["tga", "jpg", "png"] {
            let path = format!("{stem}.{ext}");
            if let Some(asset) = assets
                .read(&path, 64 * 1024 * 1024)
                .map_err(|error| error.to_string())?
            {
                let mut image = materials::decode_texture_data_with_color_space(
                    &path,
                    &asset.bytes,
                    true,
                    false,
                    false,
                )?;
                image.source = Some(asset.source);
                return Ok(Some(normalize_deluxemap(image)));
            }
        }
    }
    Ok(None)
}

pub(in crate::scene) fn embedded_lightmap_texture(index: usize, page: &[u8]) -> TextureData {
    TextureData {
        label: format!("JKA lightmap {index}"),
        source: None,
        width: 128,
        height: 128,
        rgba: (page
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect::<Vec<_>>()).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    }
}

pub(in crate::scene) fn embedded_deluxemap_texture(index: usize, page: &[u8]) -> TextureData {
    let mut rgba: Vec<u8> = page
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    for pixel in rgba.chunks_exact_mut(4) {
        if pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 {
            pixel[0] = 127;
            pixel[1] = 127;
            pixel[2] = 127;
        }
    }
    TextureData {
        label: format!("JKA deluxemap {index}"),
        source: None,
        width: 128,
        height: 128,
        rgba: (rgba).into(),
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: false,
    }
}

/// OpenJK R_ColorShiftLightingBytes. With matching map/output bits it is
/// deliberately a no-op (the ordinary JKA and JK2 automatic configurations).
/// Negative shifts use the inverse, rather than undefined signed C shifts.
pub(in crate::scene) fn overbright_rgb(rgb: [u8; 3], map_bits: u8, output_bits: u8) -> [u8; 3] {
    let difference = i32::from(map_bits) - i32::from(output_bits);
    if difference == 0 { return rgb; }
    let shift = difference.unsigned_abs().min(2);
    let mut channels = rgb.map(|value| {
        if difference > 0 { u32::from(value) << shift } else { u32::from(value) >> shift }
    });
    let peak = *channels.iter().max().unwrap_or(&0);
    if peak > 255 {
        for channel in &mut channels { *channel = *channel * 255 / peak; }
    }
    channels.map(|value| value as u8)
}

pub(in crate::scene) fn shift_lightmap_rgba(image: &mut TextureData, map_bits: u8, output_bits: u8) {
    if map_bits == output_bits || image.rgba16f.is_some() { return; }
    for pixel in image.rgba.chunks_exact_mut(4) {
        let shifted = overbright_rgb([pixel[0], pixel[1], pixel[2]], map_bits, output_bits);
        pixel[0..3].copy_from_slice(&shifted);
    }
}

#[cfg(test)]
mod overbright_tests {
    use super::overbright_rgb;
    #[test]
    fn normalizes_without_destroying_chroma() {
        assert_eq!(overbright_rgb([200, 100, 50], 1, 0), [255, 127, 63]);
        assert_eq!(overbright_rgb([32, 64, 128], 0, 1), [16, 32, 64]);
        assert_eq!(overbright_rgb([200, 100, 50], 1, 1), [200, 100, 50]);
    }
}
