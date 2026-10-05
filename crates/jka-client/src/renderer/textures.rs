//! Textures.
use crate::renderer::{
    ui, AssetSearchPath, BlendFactor, Instant, Path, TextureData, TextureFilter,
};
use wgpu::util::DeviceExt;

pub(in crate::renderer) struct GpuImage {
    pub(in crate::renderer) _texture: wgpu::Texture,
    pub(in crate::renderer) view: wgpu::TextureView,
}

pub(in crate::renderer) fn create_texture_sampler(
    device: &wgpu::Device,
    clamp: bool,

    filter: TextureFilter,
) -> wgpu::Sampler {
    let address = if clamp {
        wgpu::AddressMode::ClampToEdge
    } else {
        wgpu::AddressMode::Repeat
    };
    let (mag_filter, min_filter, mipmap_filter, lod_max_clamp) = match filter {
        TextureFilter::Nearest => (
            wgpu::FilterMode::Nearest,
            wgpu::FilterMode::Nearest,
            wgpu::MipmapFilterMode::Nearest,
            0.0,
        ),
        TextureFilter::Bilinear => (
            wgpu::FilterMode::Linear,
            wgpu::FilterMode::Linear,
            wgpu::MipmapFilterMode::Nearest,
            32.0,
        ),
        TextureFilter::Trilinear
        | TextureFilter::Anisotropic2x
        | TextureFilter::Anisotropic4x
        | TextureFilter::Anisotropic8x
        | TextureFilter::Anisotropic16x => (
            wgpu::FilterMode::Linear,
            wgpu::FilterMode::Linear,
            wgpu::MipmapFilterMode::Linear,
            32.0,
        ),
    };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(filter.label()),
        address_mode_u: address,
        address_mode_v: address,
        address_mode_w: address,
        mag_filter,
        min_filter,
        mipmap_filter,
        lod_min_clamp: 0.0,
        lod_max_clamp,
        anisotropy_clamp: filter.anisotropy_clamp(),
        ..Default::default()
    })
}

pub(in crate::renderer) fn normalize_ui_font(mut data: TextureData) -> TextureData {
    // JKA charset atlases are coverage textures. Some repacks preserve coverage in
    // alpha while others flatten it into grayscale; normalize both cases to
    // white RGB + alpha coverage once at load.
    let alpha_has_transparency = data.rgba.chunks_exact(4).any(|p| p[3] < 250);
    for pixel in data.rgba.chunks_exact_mut(4) {
        let coverage = if alpha_has_transparency {
            pixel[3]
        } else {
            pixel[0].max(pixel[1]).max(pixel[2])
        };
        pixel[0] = 255;
        pixel[1] = 255;
        pixel[2] = 255;
        pixel[3] = coverage;
    }
    data.srgb = false;
    data
}

pub(in crate::renderer) fn try_load_ui_font_asset(
    assets: &mut AssetSearchPath,
    path: &str,
) -> Option<TextureData> {
    let stem = path
        .rsplit_once('.')
        .filter(|(_, ext)| ["tga", "jpg", "jpeg", "png"].contains(ext))
        .map(|(stem, _)| stem)
        .unwrap_or(path);
    let mut candidates = vec![path.to_owned()];
    for ext in ["tga", "jpg", "png"] {
        let candidate = format!("{stem}.{ext}");
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }

    for name in candidates {
        let asset = match assets.read(&name, 8 * 1024 * 1024) {
            Ok(Some(asset)) => asset,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("UI font {name}: {error}");
                continue;
            }
        };
        match crate::materials::decode_texture_data(&name, &asset.bytes, true, false) {
            Ok(data) => {
                let data = normalize_ui_font(data);
                if crate::logging::renderer_verbose_enabled(2) {
                    crate::logging::write_line_with_path(
                        crate::logging::Level::Info,
                        format_args!(
                            "UI font: {}x{} {} from {}",
                            data.width,
                            data.height,
                            name,
                            asset.source.display()
                        ),
                        asset.source.clone(),
                    );
                }
                return Some(data);
            }
            Err(error) => eprintln!("UI font {name}: {error}"),
        }
    }
    None
}

pub(in crate::renderer) fn try_load_texture_asset_from_search_path(
    assets: &mut AssetSearchPath,
    path: &str,
    clamp: bool,
    mipmaps: bool,
    srgb: bool,
    log_prefix: &str,
) -> Option<TextureData> {
    let stem = path
        .rsplit_once('.')
        .filter(|(_, ext)| ["tga", "jpg", "jpeg", "png"].contains(ext))
        .map(|(stem, _)| stem)
        .unwrap_or(path);
    let mut candidates = vec![path.to_owned()];
    for ext in ["png", "tga", "jpg"] {
        let candidate = format!("{stem}.{ext}");
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }

    for name in candidates {
        let asset = match assets.read(&name, 16 * 1024 * 1024) {
            Ok(Some(asset)) => asset,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("{log_prefix} {name}: {error}");
                continue;
            }
        };
        match crate::materials::decode_texture_data_with_color_space(
            &name,
            &asset.bytes,
            clamp,
            mipmaps,
            srgb,
        ) {
            Ok(data) => {
                if crate::logging::renderer_verbose_enabled(2) {
                    crate::logging::write_line_with_path(
                        crate::logging::Level::Info,
                        format_args!(
                            "{log_prefix}: {}x{} {} from {}",
                            data.width,
                            data.height,
                            name,
                            asset.source.display()
                        ),
                        asset.source.clone(),
                    );
                }
                return Some(data);
            }
            Err(error) => eprintln!("{log_prefix} {name}: {error}"),
        }
    }
    None
}

pub(in crate::renderer) fn try_load_texture_asset(
    base: &Path,
    game: Option<&Path>,
    path: &str,
    clamp: bool,
    mipmaps: bool,
    srgb: bool,
    log_prefix: &str,
) -> Option<TextureData> {
    let mut assets = match AssetSearchPath::open_game(base, game) {
        Ok(assets) => assets,
        Err(error) => {
            eprintln!("{log_prefix} asset search: {error}");
            return None;
        }
    };
    try_load_texture_asset_from_search_path(&mut assets, path, clamp, mipmaps, srgb, log_prefix)
}

/// Runs one startup step and logs it when it is slow enough to matter. Used for
/// pipeline compiles, which run on worker threads where the per-phase marks in
/// `Renderer::new` cannot see them.
pub(in crate::renderer) fn timed_init_step<T>(name: &str, step: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let value = step();
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if ms >= 4.0 {
        rverbose!(1, "[RENDER INIT] {name} {ms:.1} ms");
    }
    value
}

/// `Device::create_shader_module` that logs modules slow enough to matter at
/// startup (naga parse + validation of large WGSL), so the "[RENDER INIT]" lines
/// show which module to split, trim or build off-thread.
pub(in crate::renderer) fn create_shader_module_timed(
    device: &wgpu::Device,
    descriptor: wgpu::ShaderModuleDescriptor<'_>,
) -> wgpu::ShaderModule {
    let label = descriptor.label.unwrap_or("unlabeled").to_owned();
    let started = Instant::now();
    let module = device.create_shader_module(descriptor);
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if ms >= 4.0 {
        rverbose!(1, "[RENDER INIT] shader module '{label}' {ms:.1} ms");
    }
    module
}

pub(in crate::renderer) fn load_texture_asset_or_missing(
    base: &Path,
    game: Option<&Path>,
    path: &str,
    clamp: bool,
    mipmaps: bool,
    srgb: bool,
    log_prefix: &str,
    missing: &TextureData,
) -> TextureData {
    try_load_texture_asset(base, game, path, clamp, mipmaps, srgb, log_prefix).unwrap_or_else(
        || {
            eprintln!(
            "{log_prefix}: missing {path}.png/.tga/.jpg; using shared missing texture placeholder"
        );
            missing.clone()
        },
    )
}

/// Packs the 27 `gfx/hud/keys/*` images `DF_DrawMovementKeys` draws into one
/// `ui::KEY_ATLAS_COLUMNS`-wide atlas, in `ui::MOVEMENT_KEY_ART` order. They come
/// from `japro-assets.pk3`, which is required in base, so the normal asset
/// search path finds them for every mod. A missing image leaves its cell
/// transparent.
pub(in crate::renderer) fn load_ui_key_atlas(base: &Path, game: Option<&Path>) -> TextureData {
    let cell = ui::KEY_ART_SIZE;
    let columns = ui::KEY_ATLAS_COLUMNS;
    let rows = (ui::MOVEMENT_KEY_ART.len() as u32).div_ceil(columns);
    let (width, height) = (columns * cell, rows * cell);
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    let mut missing = 0usize;
    match AssetSearchPath::open_game(base, game) {
        Ok(mut assets) => {
            for (index, name) in ui::MOVEMENT_KEY_ART.iter().enumerate() {
                let path = format!("gfx/hud/keys/{name}");
                let Some(image) = try_load_texture_asset_from_search_path(
                    &mut assets,
                    &path,
                    true,
                    false,
                    true,
                    "UI movement key",
                ) else {
                    missing += 1;
                    continue;
                };
                // jaPRO/TaystJK packs have shipped this art at different source
                // resolutions.  CG_DrawPic scales it anyway, so requiring an
                // exact 128x128 source made the whole overlay silently transparent.
                let (source_w, source_h) = (image.width, image.height);
                let Some(source) = image::RgbaImage::from_raw(source_w, source_h, image.rgba)
                else {
                    eprintln!("UI movement key {path}: unexpected pixel format");
                    missing += 1;
                    continue;
                };
                let cell_image = if source_w == cell && source_h == cell {
                    source
                } else {
                    image::imageops::resize(
                        &source,
                        cell,
                        cell,
                        image::imageops::FilterType::Triangle,
                    )
                };
                let pixels = cell_image.as_raw();
                let row_bytes = (cell * 4) as usize;
                let (column, row) = (index as u32 % columns, index as u32 / columns);
                for y in 0..cell {
                    let src = y as usize * row_bytes;
                    let dst = (((row * cell + y) * width + column * cell) * 4) as usize;
                    rgba[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
                }
            }
        }
        Err(error) => {
            eprintln!("UI movement keys asset search: {error}");
            missing = ui::MOVEMENT_KEY_ART.len();
        }
    }
    if missing > 0 {
        eprintln!("UI movement keys: {missing} gfx/hud/keys image(s) not found (japro-assets.pk3 belongs in base)");
    }
    TextureData {
        label: "JKA UI movement key atlas".into(),
        source: None,
        width,
        height,
        rgba,
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    }
}

pub(in crate::renderer) fn rgb565(rgb: [u8; 3]) -> u16 {
    (u16::from(rgb[0] >> 3) << 11) | (u16::from(rgb[1] >> 2) << 5) | u16::from(rgb[2] >> 3)
}

pub(in crate::renderer) fn rgb565_expand(value: u16) -> [u8; 3] {
    let r = ((value >> 11) & 31) as u8;
    let g = ((value >> 5) & 63) as u8;
    let b = (value & 31) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

pub(in crate::renderer) fn encode_bc3_block(pixels: &[[u8; 4]; 16], output: &mut Vec<u8>) {
    // DXT5/BC3 alpha block.
    let alpha_max = pixels.iter().map(|p| p[3]).max().unwrap_or(255);
    let alpha_min = pixels.iter().map(|p| p[3]).min().unwrap_or(0);
    output.push(alpha_max);
    output.push(alpha_min);
    let mut alpha_palette = [0u8; 8];
    alpha_palette[0] = alpha_max;
    alpha_palette[1] = alpha_min;
    if alpha_max > alpha_min {
        for i in 1..=6usize {
            alpha_palette[i + 1] =
                (((7 - i) as u16 * u16::from(alpha_max) + i as u16 * u16::from(alpha_min) + 3) / 7)
                    as u8;
        }
    } else {
        for i in 1..=4usize {
            alpha_palette[i + 1] =
                (((5 - i) as u16 * u16::from(alpha_max) + i as u16 * u16::from(alpha_min) + 2) / 5)
                    as u8;
        }
        alpha_palette[6] = 0;
        alpha_palette[7] = 255;
    }
    let mut alpha_bits = 0u64;
    for (i, pixel) in pixels.iter().enumerate() {
        let mut best = 0usize;
        let mut best_error = u16::MAX;
        for (candidate, value) in alpha_palette.iter().enumerate() {
            let error = (i16::from(pixel[3]) - i16::from(*value)).unsigned_abs();
            if error < best_error {
                best_error = error;
                best = candidate;
            }
        }
        alpha_bits |= (best as u64) << (3 * i);
    }
    for byte in 0..6 {
        output.push(((alpha_bits >> (8 * byte)) & 0xff) as u8);
    }

    // BC3's RGB block always uses the four-colour BC1 interpolation mode.
    // Companion maps are data, not photographs, so preserve the full per-channel
    // range rather than choosing endpoints only from luminance extrema.
    let mut minimum = [255u8; 3];
    let mut maximum = [0u8; 3];
    for pixel in pixels {
        for channel in 0..3 {
            minimum[channel] = minimum[channel].min(pixel[channel]);
            maximum[channel] = maximum[channel].max(pixel[channel]);
        }
    }
    let mut color0 = rgb565(maximum);
    let mut color1 = rgb565(minimum);
    if color0 < color1 {
        std::mem::swap(&mut color0, &mut color1);
    }
    output.extend_from_slice(&color0.to_le_bytes());
    output.extend_from_slice(&color1.to_le_bytes());
    let c0 = rgb565_expand(color0);
    let c1 = rgb565_expand(color1);
    let palette = [
        c0,
        c1,
        [
            ((2 * u16::from(c0[0]) + u16::from(c1[0]) + 1) / 3) as u8,
            ((2 * u16::from(c0[1]) + u16::from(c1[1]) + 1) / 3) as u8,
            ((2 * u16::from(c0[2]) + u16::from(c1[2]) + 1) / 3) as u8,
        ],
        [
            ((u16::from(c0[0]) + 2 * u16::from(c1[0]) + 1) / 3) as u8,
            ((u16::from(c0[1]) + 2 * u16::from(c1[1]) + 1) / 3) as u8,
            ((u16::from(c0[2]) + 2 * u16::from(c1[2]) + 1) / 3) as u8,
        ],
    ];
    let mut color_bits = 0u32;
    for (i, pixel) in pixels.iter().enumerate() {
        let mut best = 0usize;
        let mut best_error = u32::MAX;
        for (candidate, color) in palette.iter().enumerate() {
            let dr = i32::from(pixel[0]) - i32::from(color[0]);
            let dg = i32::from(pixel[1]) - i32::from(color[1]);
            let db = i32::from(pixel[2]) - i32::from(color[2]);
            let error = (dr * dr + dg * dg + db * db) as u32;
            if error < best_error {
                best_error = error;
                best = candidate;
            }
        }
        color_bits |= (best as u32) << (2 * i);
    }
    output.extend_from_slice(&color_bits.to_le_bytes());
}

pub(in crate::renderer) fn mip_upload_window(
    data: &TextureData,
    picmip: u32,
    bytes_per_texel: usize,
) -> (u32, u32, u32, usize) {
    let levels = data.mip_level_count.max(1);
    let skip = picmip.min(levels.saturating_sub(1));
    let mut width = data.width.max(1);
    let mut height = data.height.max(1);
    let mut offset = 0usize;
    for _ in 0..skip {
        offset = offset.saturating_add(width as usize * height as usize * bytes_per_texel);
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    (width, height, levels - skip, offset)
}

pub(in crate::renderer) fn encode_bc3_mip_chain(data: &TextureData, picmip: u32) -> Vec<u8> {
    let mut encoded = Vec::new();
    let (mut width, mut height, mip_level_count, mut source_offset) =
        mip_upload_window(data, picmip, 4);
    for _ in 0..mip_level_count {
        let source_len = width as usize * height as usize * 4;
        let source = &data.rgba[source_offset..source_offset + source_len];
        for block_y in 0..height.div_ceil(4) {
            for block_x in 0..width.div_ceil(4) {
                let mut block = [[0u8; 4]; 16];
                for py in 0..4u32 {
                    for px in 0..4u32 {
                        let x = (block_x * 4 + px).min(width - 1);
                        let y = (block_y * 4 + py).min(height - 1);
                        let src = ((y * width + x) * 4) as usize;
                        let dst = (py * 4 + px) as usize;
                        block[dst].copy_from_slice(&source[src..src + 4]);
                    }
                }
                encode_bc3_block(&block, &mut encoded);
            }
        }
        source_offset += source_len;
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    encoded
}

pub(in crate::renderer) fn upload_texture_bc3_picmip(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &TextureData,
    picmip: u32,
) -> GpuImage {
    let (width, height, mip_level_count, _) = mip_upload_window(data, picmip, 4);
    let encoded = encode_bc3_mip_chain(data, picmip);
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(&format!("{} [BC3 companion]", data.label)),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bc3RgbaUnorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &encoded,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuImage {
        _texture: texture,
        view,
    }
}

pub(in crate::renderer) fn upload_texture_bc3(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &TextureData,
) -> GpuImage {
    upload_texture_bc3_picmip(device, queue, data, 0)
}

pub(in crate::renderer) fn upload_texture_picmip(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &TextureData,
    picmip: u32,
) -> GpuImage {
    // `r_picmip` is a map colour/material texture setting here (TaystJK's
    // default smart-picmip policy). FP16 lightmaps and other explicit renderer
    // resources continue through upload_texture without picmip.
    if data.rgba16f.is_some() {
        return upload_texture(device, queue, data);
    }
    let (width, height, mip_level_count, offset) = mip_upload_window(data, picmip, 4);
    let format = if data.srgb {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };
    let bytes = data.rgba.get(offset..).unwrap_or(&data.rgba);
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(&data.label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | if format == wgpu::TextureFormat::Rgba8UnormSrgb {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytes,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuImage {
        _texture: texture,
        view,
    }
}

pub(in crate::renderer) fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &TextureData,
) -> GpuImage {
    let format = if data.rgba16f.is_some() {
        wgpu::TextureFormat::Rgba16Float
    } else if data.srgb {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };
    let bytes: &[u8] = data
        .rgba16f
        .as_deref()
        .map(bytemuck::cast_slice)
        .unwrap_or(&data.rgba);
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(&data.label),
            size: wgpu::Extent3d {
                width: data.width,
                height: data.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: data.mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | if format == wgpu::TextureFormat::Rgba8UnormSrgb {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytes,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuImage {
        _texture: texture,
        view,
    }
}

pub(in crate::renderer) fn gpu_blend_factor(factor: BlendFactor) -> wgpu::BlendFactor {
    match factor {
        BlendFactor::Zero => wgpu::BlendFactor::Zero,
        BlendFactor::One => wgpu::BlendFactor::One,
        BlendFactor::SrcColor => wgpu::BlendFactor::Src,
        BlendFactor::OneMinusSrcColor => wgpu::BlendFactor::OneMinusSrc,
        BlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        BlendFactor::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        BlendFactor::DstColor => wgpu::BlendFactor::Dst,
        BlendFactor::OneMinusDstColor => wgpu::BlendFactor::OneMinusDst,
        BlendFactor::DstAlpha => wgpu::BlendFactor::DstAlpha,
        BlendFactor::OneMinusDstAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
        BlendFactor::SrcAlphaSaturate => wgpu::BlendFactor::SrcAlphaSaturated,
    }
}
