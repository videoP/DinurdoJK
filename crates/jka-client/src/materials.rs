//! JKA/Q3 material interpretation for the direct wgpu renderer.
//!
//! The common lightmapped path stays compact, while explicit shader scripts retain
//! their ordered stages so environment maps, alpha overlays, lightmap filters, and
//! texture-coordinate generation can be reproduced without shader-name hacks.
use crate::{map_jobs::MapJobPool, thread_activity::Task};
use image::ImageFormat;
use jka_assets::{
    bsp::{MATERIAL_MASK, SURF_NODRAW, SURF_SKY},
    pk3::AssetSearchPath,
    shader::{self, AlphaGen, RgbGen, Shader, TcGen, TcMod},
};
use std::{collections::BTreeMap, path::{Path, PathBuf}, time::Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstColor,
    OneMinusDstColor,
    DstAlpha,
    OneMinusDstAlpha,
    SrcAlphaSaturate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlendFunc {
    pub src: BlendFactor,
    pub dst: BlendFactor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CullMode {
    None,
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageTexture {
    Image(usize),
    Lightmap,
    White,
}

#[derive(Debug, Clone)]
pub struct StageEnhancements {
    pub normal_texture: Option<usize>,
    pub roughness_texture: Option<usize>,
    pub height_texture: Option<usize>,
    pub metallic_texture: Option<usize>,
    pub specular_texture: Option<usize>,
    pub emissive_texture: Option<usize>,
    /// Rend2 normalHeightMap stores height in alpha instead of a standalone map.
    pub height_from_alpha: bool,
    /// Rend2 rmoMap packs roughness=R, metalness=G, occlusion=B.
    pub rmo_packed: bool,
    /// Rend2 rmosMap additionally stores per-pixel specular scale in alpha.
    pub rmo_specular_alpha: bool,
    /// Rend2 normalScale. Defaults to [1, 1].
    pub normal_scale: [f32; 2],
    /// Fixed stage roughness/gloss override. None means use the texture/default.
    pub roughness_override: Option<f32>,
    /// Fixed linear-space dielectric reflectance override.
    pub specular_reflectance: Option<[f32; 3]>,
    /// Authored Rend2 parallaxDepth. Zero means use the renderer default.
    pub parallax_depth: f32,
}

impl Default for StageEnhancements {
    fn default() -> Self {
        Self {
            normal_texture: None,
            roughness_texture: None,
            height_texture: None,
            metallic_texture: None,
            specular_texture: None,
            emissive_texture: None,
            height_from_alpha: false,
            rmo_packed: false,
            rmo_specular_alpha: false,
            normal_scale: [1.0, 1.0],
            roughness_override: None,
            specular_reflectance: None,
            parallax_depth: 0.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MaterialStage {
    pub texture: StageTexture,
    pub enhancements: StageEnhancements,
    pub blend: Option<BlendFunc>,
    pub alpha_cutoff: f32,
    pub opacity: f32,
    pub color: [f32; 3],
    pub rgb_gen: RgbGen,
    pub alpha_gen: AlphaGen,
    pub tc_gen: TcGen,
    pub tc_mods: Vec<TcMod>,
    pub depth_write: bool,
    pub depth_equal: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceLight {
    /// q3map_surfacelight-style intensity used by the runtime area-light extractor.
    pub value: f32,
    /// Normalized emitter chromaticity, preferably from q3map_lightimage.
    pub color: [f32; 3],
    /// q3map_lightsubdivide, or q3map2's classic 120-unit default.
    pub subdivide: f32,
    /// True when this emitter was inferred from an emissive companion texture
    /// instead of explicit q3map_surfacelight metadata.
    pub inferred_from_emissive: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct GrassMaterial {
    /// Original JKA sprite-card width. Kept as an authored scale hint; the
    /// modern grass blade keeps GodotGrass' much slimmer blade aspect ratio.
    pub authored_width: f32,
    pub height: f32,
    /// JKA surfaceSprites density is a spacing-like authoring parameter.
    pub density: f32,
    pub fade_dist: f32,
    pub fade_max: f32,
    pub wind: f32,
    /// ssAnyAngle explicitly permits emitter triangles outside the normal
    /// ground-slope filter.
    pub any_angle: bool,
}

/// One JKA `surfaceSprites effect` stage. Unlike an ordinary material pass,
/// Raven's backend expands this stage into camera-facing QuickSprite quads at
/// runtime; the source surface UVs are never used for the splash texture.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceSpriteEffectMaterial {
    pub texture: usize,
    pub clamp: bool,
    pub blend: Option<BlendFunc>,
    pub sprite: shader::SurfaceSprite,
}

#[derive(Debug, Clone)]
pub struct SurfaceMaterial {
    pub stages: Vec<MaterialStage>,
    pub surface_material: u8,
    pub cull: CullMode,
    pub offset: bool,
    /// Authored id Tech 3 `portal` / `sort portal` surface. These surfaces are
    /// eligible for the renderer's exact planar-reflection path.
    pub planar_reflection: bool,
    /// True only for authored `surfaceparm water`.
    pub water: bool,
    /// q3map2 `surfaceparm alphashadow`.
    pub alpha_shadow: bool,
    /// q3map2 `surfaceparm lightfilter`.
    pub light_filter: bool,
    /// q3map2 `q3map_nofog`: compiler metadata saying this material must not
    /// participate in local/global BSP fog.
    pub no_fog: bool,
    pub sky: bool,
    pub skybox: Option<[usize; 6]>,
    pub hidden: bool,
    /// Authored q3map area emitter metadata. This is compile-time material
    /// information and is independent of whether the visible stage has an
    /// emissive companion texture.
    pub surface_light: Option<SurfaceLight>,
    /// Procedural per-blade grass emitted by a JKA `surfaceSprites vertical` stage.
    pub grass: Option<GrassMaterial>,
    /// Raven QuickSprite effect stages retained separately from ordinary BSP
    /// material passes so they can be distributed over triangles at runtime.
    pub surface_sprite_effects: Vec<SurfaceSpriteEffectMaterial>,
    /// Retail JKA's quick-sprite path finishes by re-enabling face culling even
    /// when the parent shader authored `cull twosided`. wgpu has no mutable GL
    /// cull state to leak between passes, so scene preparation uses this bit to
    /// reproduce the visible retail result deterministically for world geometry.
    pub surface_sprite_cull_quirk: bool,
    /// False for the implicit "texture + map lighting" material synthesized when
    /// no shader script exists. This lets scene preparation keep that path in one
    /// draw call while explicit multi-pass scripts remain ordered passes.
    pub explicit: bool,
}

impl Default for SurfaceMaterial {
    fn default() -> Self {
        Self {
            stages: Vec::new(),
            surface_material: 0,
            cull: CullMode::Back,
            offset: false,
            planar_reflection: false,
            water: false,
            alpha_shadow: false,
            light_filter: false,
            no_fog: false,
            sky: false,
            skybox: None,
            hidden: false,
            surface_light: None,
            grass: None,
            surface_sprite_effects: Vec::new(),
            surface_sprite_cull_quirk: false,
            explicit: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextureData {
    pub label: String,
    /// Physical PK3/ZIP/loose file that supplied this image. Generated images
    /// use `None`; derived images inherit the source of their base texture.
    pub source: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// All mip levels, largest to smallest, tightly packed. For non-mipmapped
    /// images this contains only the base level.
    pub rgba: Vec<u8>,
    /// Optional native FP16 texels, RGBA channel order. Used by HDR lightmaps.
    /// When present, upload ignores `rgba` for rendering but keeps it as an
    /// inspector/AO fallback copy.
    pub rgba16f: Option<Vec<u16>>,
    pub mip_level_count: u32,
    pub clamp: bool,
    /// Color textures use sRGB sampling; data maps stay linear.
    pub srgb: bool,
}

/// Shared missing-material placeholder used by renderer paths that resolve a
/// real shader/image qpath. Missing assets must remain visibly broken instead
/// of silently turning into the white fallback used for intentional $whiteimage.
pub fn missing_texture_data() -> TextureData {
    const SIZE: usize = 16;
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let index = (y * SIZE + x) * 4;
            let border = x == 0 || y == 0 || x == SIZE - 1 || y == SIZE - 1;
            let checker = ((x / 4) + (y / 4)) % 2 == 0;
            let color = if border {
                [0_u8, 0_u8, 0_u8, 255_u8]
            } else if checker {
                [255_u8, 0_u8, 255_u8, 255_u8]
            } else {
                [0_u8, 0_u8, 0_u8, 255_u8]
            };
            rgba[index..index + 4].copy_from_slice(&color);
        }
    }
    TextureData {
        label: "missing texture fallback".to_owned(),
        source: None,
        width: SIZE as u32,
        height: SIZE as u32,
        rgba,
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    }
}

#[derive(Debug, Clone)]
pub struct ShaderDefinitionOrigin {
    pub file: String,
    /// Physical PK3/ZIP/loose file that supplied this definition. Kept so
    /// PBR base-color replacements can be scoped to the material package that
    /// authored them instead of becoming unconditional global overrides.
    pub source: PathBuf,
    pub mtr_override: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ShaderLibraryDiagnostics {
    pub shader_files: usize,
    pub mtr_files: usize,
    pub shader_definitions: usize,
    pub mtr_definitions: usize,
    pub origins: BTreeMap<String, ShaderDefinitionOrigin>,
    /// Time spent pulling shader text out of PK3/loose assets.
    pub read_ms: f64,
    /// Sum of CPU time spent parsing individual shader files. With workers this
    /// can exceed wall time because several files parse concurrently.
    pub parse_cpu_ms: f64,
    /// Wall time from the first parse submission until all parses completed.
    pub parse_wall_ms: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TextureLoadStats {
    pub preload_wall_ms: f64,
    pub read_ms: f64,
    pub decode_ms: f64,
    pub mip_ms: f64,
    pub decoded_images: usize,
}

pub fn shader_library(
    assets: &mut AssetSearchPath,
    warnings: &mut Vec<String>,
    enable_mtr: bool,
) -> Result<(BTreeMap<String, Shader>, ShaderLibraryDiagnostics), String> {
    shader_library_internal(assets, warnings, None, enable_mtr)
}

pub fn shader_library_with_jobs(
    assets: &mut AssetSearchPath,
    warnings: &mut Vec<String>,
    jobs: &MapJobPool,
    enable_mtr: bool,
) -> Result<(BTreeMap<String, Shader>, ShaderLibraryDiagnostics), String> {
    shader_library_internal(assets, warnings, Some(jobs), enable_mtr)
}

fn shader_library_internal(
    assets: &mut AssetSearchPath,
    warnings: &mut Vec<String>,
    jobs: Option<&MapJobPool>,
    enable_mtr: bool,
) -> Result<(BTreeMap<String, Shader>, ShaderLibraryDiagnostics), String> {
    // Rend2 .mtr files are renderer-specific material overrides. Load ordinary
    // .shader definitions first, then replace matching definitions from .mtr.
    // File IO remains serialized through one AssetSearchPath (ZipArchive is
    // stateful); the CPU-heavy tokenization/parser work can run on map workers.
    let shader_files: Vec<_> = assets
        .names()
        .filter(|name| name.starts_with("shaders/") && name.ends_with(".shader"))
        .map(str::to_owned)
        .collect();
    let available_material_files: Vec<_> = assets
        .names()
        .filter(|name| name.starts_with("shaders/") && name.ends_with(".mtr"))
        .map(str::to_owned)
        .collect();
    let material_files: Vec<_> = if enable_mtr {
        available_material_files.clone()
    } else {
        Vec::new()
    };

    let mut diagnostics = ShaderLibraryDiagnostics {
        shader_files: shader_files.len(),
        mtr_files: material_files.len(),
        ..Default::default()
    };

    struct ParsedFile {
        name: String,
        source: PathBuf,
        override_existing: bool,
        parsed: Result<BTreeMap<String, Shader>, String>,
        parse_ms: f64,
    }

    let parse_wall_started = Instant::now();
    let mut parsed_files = Vec::with_capacity(shader_files.len() + material_files.len());
    if let Some(jobs) = jobs {
        let mut handles = Vec::with_capacity(shader_files.len() + material_files.len());
        for (files, override_existing) in [(&shader_files, false), (&material_files, true)] {
            for name in files {
                let read_started = Instant::now();
                let asset = assets
                    .read(name, 8 * 1024 * 1024)
                    .map_err(|e| e.to_string())?
                    .ok_or("indexed material definition vanished")?;
                diagnostics.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
                let name = name.clone();
                let source = asset.source;
                let bytes = asset.bytes;
                handles.push(jobs.submit(Task::MapShaderParse, move || {
                    let started = Instant::now();
                    let parsed = shader::parse(&String::from_utf8_lossy(&bytes));
                    ParsedFile {
                        name,
                        source,
                        override_existing,
                        parsed,
                        parse_ms: started.elapsed().as_secs_f64() * 1000.0,
                    }
                })?);
            }
        }
        // Join in submission order so ordinary shader precedence remains byte-for-byte
        // compatible with the old sequential loader even though parsing ran in parallel.
        for handle in handles {
            parsed_files.push(handle.join()?);
        }
    } else {
        for (files, override_existing) in [(&shader_files, false), (&material_files, true)] {
            for name in files {
                let read_started = Instant::now();
                let asset = assets
                    .read(name, 8 * 1024 * 1024)
                    .map_err(|e| e.to_string())?
                    .ok_or("indexed material definition vanished")?;
                diagnostics.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
                let started = Instant::now();
                let parsed = shader::parse(&String::from_utf8_lossy(&asset.bytes));
                parsed_files.push(ParsedFile {
                    name: name.clone(),
                    source: asset.source,
                    override_existing,
                    parsed,
                    parse_ms: started.elapsed().as_secs_f64() * 1000.0,
                });
            }
        }
    }
    diagnostics.parse_wall_ms = parse_wall_started.elapsed().as_secs_f64() * 1000.0;

    let mut shaders = BTreeMap::new();
    for file in parsed_files {
        diagnostics.parse_cpu_ms += file.parse_ms;
        match file.parsed {
            Ok(parsed) => {
                if file.override_existing {
                    diagnostics.mtr_definitions += parsed.len();
                } else {
                    diagnostics.shader_definitions += parsed.len();
                }
                for (shader_name, shader) in parsed {
                    if file.override_existing {
                        shaders.insert(shader_name.clone(), shader);
                        diagnostics.origins.insert(
                            shader_name,
                            ShaderDefinitionOrigin {
                                file: file.name.clone(),
                                source: file.source.clone(),
                                mtr_override: true,
                            },
                        );
                    } else if let std::collections::btree_map::Entry::Vacant(entry) =
                        shaders.entry(shader_name.clone())
                    {
                        entry.insert(shader);
                        diagnostics.origins.insert(
                            shader_name,
                            ShaderDefinitionOrigin {
                                file: file.name.clone(),
                                source: file.source.clone(),
                                mtr_override: false,
                            },
                        );
                    }
                }
            }
            Err(error) => warnings.push(format!("{}: {error}", file.name)),
        }
    }
    if !material_files.is_empty() {
        warnings.push(format!(
            "Rend2: loaded {} .mtr file(s), {} material definition(s)",
            material_files.len(),
            diagnostics.mtr_definitions
        ));
    } else if !enable_mtr && !available_material_files.is_empty() {
        warnings.push(format!(
            "Rend2: r_pbr is off; ignored {} .mtr file(s) and kept normal JKA .shader/implicit materials",
            available_material_files.len()
        ));
    }
    Ok((shaders, diagnostics))
}

pub struct Textures {
    pub images: Vec<TextureData>,
    cache: BTreeMap<(String, bool, bool), Option<usize>>,
    /// Source-affine cache for active `.mtr` dependencies. The same qpath may
    /// legitimately resolve to a different image when owned by a different
    /// material provider, so it must not alias the ordinary global VFS cache.
    source_cache: BTreeMap<(String, bool, bool, PathBuf), Option<usize>>,
    generated_normal_cache: BTreeMap<usize, usize>,
    pub warnings: Vec<String>,
    pub load_stats: TextureLoadStats,
}

impl Textures {
    pub fn new() -> Self {
        Self {
            images: Vec::new(),
            cache: BTreeMap::new(),
            source_cache: BTreeMap::new(),
            generated_normal_cache: BTreeMap::new(),
            warnings: Vec::new(),
            load_stats: TextureLoadStats::default(),
        }
    }

    pub fn load(&mut self, assets: &mut AssetSearchPath, path: &str, clamp: bool) -> Option<usize> {
        let path = path.replace('\\', "/").to_ascii_lowercase();
        if let Some(value) = self.cache.get(&(path.clone(), clamp, true)) {
            return *value;
        }
        let result = self.decode(assets, &path, clamp, true);
        let value = match result {
            Ok(image) => {
                let index = self.images.len();
                self.images.push(image);
                Some(index)
            }
            Err(error) => {
                self.warnings.push(error);
                None
            }
        };
        self.cache.insert((path, clamp, true), value);
        value
    }

    /// Resolve an active material dependency from the same physical provider as
    /// the `.mtr` first, then fall back to ordinary JKA VFS lookup if that
    /// provider does not contain the requested image.
    pub fn load_from_source(
        &mut self,
        assets: &mut AssetSearchPath,
        path: &str,
        clamp: bool,
        source: &Path,
    ) -> Option<usize> {
        self.load_with_source(assets, path, clamp, true, source, true)
    }

    pub fn load_optional_linear_from_source(
        &mut self,
        assets: &mut AssetSearchPath,
        path: &str,
        clamp: bool,
        source: &Path,
    ) -> Option<usize> {
        self.load_with_source(assets, path, clamp, false, source, false)
    }

    pub fn load_optional_srgb_from_source(
        &mut self,
        assets: &mut AssetSearchPath,
        path: &str,
        clamp: bool,
        source: &Path,
    ) -> Option<usize> {
        self.load_with_source(assets, path, clamp, true, source, false)
    }

    fn load_with_source(
        &mut self,
        assets: &mut AssetSearchPath,
        path: &str,
        clamp: bool,
        srgb: bool,
        source: &Path,
        warn: bool,
    ) -> Option<usize> {
        let path = path.replace('\\', "/").to_ascii_lowercase();
        let key = (path.clone(), clamp, srgb, source.to_path_buf());
        if let Some(value) = self.source_cache.get(&key) {
            return *value;
        }
        let result = self.decode_from_source(assets, &path, clamp, srgb, source);
        let value = match result {
            Ok(image) => {
                let index = self.images.len();
                self.images.push(image);
                Some(index)
            }
            Err(error) => {
                if warn {
                    self.warnings.push(error);
                }
                None
            }
        };
        self.source_cache.insert(key, value);
        value
    }

    /// Load an optional linear-data texture without emitting a missing-texture warning.
    pub fn load_optional_linear(
        &mut self,
        assets: &mut AssetSearchPath,
        path: &str,
        clamp: bool,
    ) -> Option<usize> {
        let path = path.replace('\\', "/").to_ascii_lowercase();
        if let Some(value) = self.cache.get(&(path.clone(), clamp, false)) {
            return *value;
        }
        let value = self.decode(assets, &path, clamp, false).ok().map(|image| {
            let index = self.images.len();
            self.images.push(image);
            index
        });
        self.cache.insert((path, clamp, false), value);
        value
    }

    /// Load an optional sRGB companion texture without emitting a missing-texture warning.
    pub fn load_optional_srgb(
        &mut self,
        assets: &mut AssetSearchPath,
        path: &str,
        clamp: bool,
    ) -> Option<usize> {
        let path = path.replace('\\', "/").to_ascii_lowercase();
        if let Some(value) = self.cache.get(&(path.clone(), clamp, true)) {
            return *value;
        }
        let value = self.decode(assets, &path, clamp, true).ok().map(|image| {
            let index = self.images.len();
            self.images.push(image);
            index
        });
        self.cache.insert((path, clamp, true), value);
        value
    }

    pub fn preload(
        &mut self,
        assets: &mut AssetSearchPath,
        requests: &[(String, bool, bool)],
        jobs: &MapJobPool,
    ) -> Result<(), String> {
        let preload_started = Instant::now();
        let mut pending = Vec::new();
        for (path, clamp, srgb) in requests {
            let path = path.replace('\\', "/").to_ascii_lowercase();
            let key = (path.clone(), *clamp, *srgb);
            if self.cache.contains_key(&key) {
                continue;
            }
            let stem = path
                .rsplit_once('.')
                .filter(|(_, ext)| ["tga", "jpg", "jpeg", "png"].contains(ext))
                .map(|(stem, _)| stem)
                .unwrap_or(path.as_str());
            let mut candidates = vec![path.clone()];
            for ext in ["tga", "jpg", "png"] {
                let candidate = format!("{stem}.{ext}");
                if !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }

            let mut found = None;
            if !assets.asset_overrides_allowed() {
                for name in &candidates {
                    let read_started = Instant::now();
                    let asset = assets
                        .read_stock(name, 64 * 1024 * 1024)
                        .map_err(|e| e.to_string())?;
                    self.load_stats.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
                    if let Some(asset) = asset {
                        found = Some((name.clone(), asset.source, asset.bytes));
                        break;
                    }
                }
            }
            if found.is_none() {
                for name in candidates {
                    let read_started = Instant::now();
                    let asset = assets
                        .read(&name, 64 * 1024 * 1024)
                        .map_err(|e| e.to_string())?;
                    self.load_stats.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
                    if let Some(asset) = asset {
                        found = Some((name, asset.source, asset.bytes));
                        break;
                    }
                }
            }
            let Some((name, source, bytes)) = found else {
                continue;
            };
            let clamp = *clamp;
            let srgb = *srgb;
            let handle = jobs.submit(Task::MapTextureDecode, move || {
                let result = decode_texture_data_profiled(&name, &bytes, clamp, true, srgb)
                    .map(|(mut image, decode_ms, mip_ms)| {
                        image.source = Some(source);
                        (image, decode_ms, mip_ms)
                    });
                (key, result)
            })?;
            pending.push(handle);
        }

        // Resolve in request order so texture indices remain deterministic.
        for handle in pending {
            let (key, result) = handle.join()?;
            match result {
                Ok((image, decode_ms, mip_ms)) => {
                    let index = self.images.len();
                    self.images.push(image);
                    self.cache.insert(key, Some(index));
                    self.load_stats.decode_ms += decode_ms;
                    self.load_stats.mip_ms += mip_ms;
                    self.load_stats.decoded_images += 1;
                }
                Err(error) => {
                    self.warnings.push(error);
                    self.cache.insert(key, None);
                }
            }
        }
        self.load_stats.preload_wall_ms += preload_started.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }


    /// Rend2 compatibility path: port of rd-rend2 RGBAtoNormal().
    /// Converts diffuse RGB to a height field, levels it, then runs the same
    /// 3x3 Sobel filter used by OpenJK/Rend2. Alpha retains the generated
    /// height channel just like IMGTYPE_NORMALHEIGHT.
    pub fn generated_normal(&mut self, base_index: usize) -> Option<usize> {
        if let Some(&index) = self.generated_normal_cache.get(&base_index) {
            return Some(index);
        }
        let base = self.images.get(base_index)?.clone();
        let width = base.width as usize;
        let height = base.height as usize;
        if width == 0 || height == 0 || base.rgba.len() < width * height * 4 {
            return None;
        }

        let mut rgba = vec![0_u8; width * height * 4];

        // OpenJK/Rend2 RGBAtoNormal(): convert RGB to its integer Y-like height
        // channel, square it to approximate linear intensity, and level the
        // maximum to 255 before taking derivatives. Do this on the source bytes
        // exactly; the reference generator operates before sRGB GPU sampling.
        let mut maximum = 1_u8;
        for (src, dst) in base.rgba[..width * height * 4]
            .chunks_exact(4)
            .zip(rgba.chunks_exact_mut(4))
        {
            let y = (src[0] >> 2) as u16 + (src[1] >> 1) as u16 + (src[2] >> 2) as u16;
            let linear = ((y * y) / 255).min(255) as u8;
            dst[3] = linear;
            maximum = maximum.max(linear);
        }
        if maximum < 255 {
            let lift = 255_u8 - maximum;
            for pixel in rgba.chunks_exact_mut(4) {
                pixel[3] = pixel[3].saturating_add(lift);
            }
        }

        let coord = |value: isize, limit: usize, clamp: bool| -> usize {
            if clamp {
                value.clamp(0, limit.saturating_sub(1) as isize) as usize
            } else {
                value.rem_euclid(limit.max(1) as isize) as usize
            }
        };
        let encode = |value: f32| -> u8 {
            // Equivalent to Rend2 FloatToOffsetByte for normalized [-1, 1].
            ((value * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
        };

        // Run the reference 3x3 Sobel filter over the generated alpha heights.
        let heights = rgba.clone();
        for y in 0..height {
            for x in 0..width {
                let mut s = [0_f32; 9];
                let mut n = 0;
                for oy in -1_isize..=1 {
                    let sy = coord(y as isize + oy, height, base.clamp);
                    for ox in -1_isize..=1 {
                        let sx = coord(x as isize + ox, width, base.clamp);
                        s[n] = f32::from(heights[(sy * width + sx) * 4 + 3]);
                        n += 1;
                    }
                }
                let nx = s[0] - s[2] + 2.0 * s[3] - 2.0 * s[5] + s[6] - s[8];
                let ny = s[0] + 2.0 * s[1] + s[2] - s[6] - 2.0 * s[7] - s[8];
                let nz = s[4] * 4.0;
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let normal = if len > 1.0e-8 {
                    [nx / len, ny / len, nz / len]
                } else {
                    [0.0, 0.0, 1.0]
                };
                let i = (y * width + x) * 4;
                rgba[i] = encode(normal[0]);
                rgba[i + 1] = encode(normal[1]);
                rgba[i + 2] = encode(normal[2]);
                // rgba[i + 3] intentionally remains the generated height.
            }
        }

        let (rgba, mip_level_count) = build_normal_height_mip_chain(base.width, base.height, rgba);
        let index = self.images.len();
        self.images.push(TextureData {
            label: format!("{} [Rend2 generated normal]", base.label),
            source: base.source.clone(),
            width: base.width,
            height: base.height,
            rgba,
            rgba16f: None,
            mip_level_count,
            clamp: base.clamp,
            srgb: false,
        });
        self.generated_normal_cache.insert(base_index, index);
        Some(index)
    }

    pub fn load_skybox(
        &mut self,
        assets: &mut AssetSearchPath,
        prefix: &str,
    ) -> Option<[usize; 6]> {
        // JKA/Q3 outerbox load order. tr_sky's sky_texorder swaps the middle
        // axes when generating geometry; keeping this file order lets the WGSL
        // face selection follow the same rt/bk/lf/ft/up/dn convention.
        let suffixes = ["rt", "bk", "lf", "ft", "up", "dn"];
        let mut result = [0usize; 6];
        for (i, suffix) in suffixes.into_iter().enumerate() {
            let path = format!("{prefix}_{suffix}");
            let Some(index) = self.load(assets, &path, true) else {
                self.warnings
                    .push(format!("Skybox {prefix}: missing face {path}"));
                return None;
            };
            result[i] = index;
        }
        Some(result)
    }

    pub fn load_skybox_from_source(
        &mut self,
        assets: &mut AssetSearchPath,
        prefix: &str,
        source: &Path,
    ) -> Option<[usize; 6]> {
        let suffixes = ["rt", "bk", "lf", "ft", "up", "dn"];
        let mut result = [0usize; 6];
        for (i, suffix) in suffixes.into_iter().enumerate() {
            let path = format!("{prefix}_{suffix}");
            let Some(index) = self.load_from_source(assets, &path, true, source) else {
                self.warnings
                    .push(format!("Skybox {prefix}: missing face {path}"));
                return None;
            };
            result[i] = index;
        }
        Some(result)
    }

    fn decode_from_source(
        &mut self,
        assets: &mut AssetSearchPath,
        path: &str,
        clamp: bool,
        srgb: bool,
        source: &Path,
    ) -> Result<TextureData, String> {
        let stem = path
            .rsplit_once('.')
            .filter(|(_, ext)| ["tga", "jpg", "jpeg", "png"].contains(ext))
            .map(|(stem, _)| stem)
            .unwrap_or(path);
        let mut candidates = vec![path.to_string()];
        for ext in ["tga", "jpg", "png"] {
            let candidate = format!("{stem}.{ext}");
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }

        for name in &candidates {
            let read_started = Instant::now();
            let asset = assets
                .read_from_source(name, 64 * 1024 * 1024, source)
                .map_err(|e| e.to_string())?;
            self.load_stats.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
            let Some(asset) = asset else {
                continue;
            };
            let (mut image, decode_ms, mip_ms) =
                decode_texture_data_profiled(name, &asset.bytes, clamp, true, srgb)?;
            image.source = Some(asset.source);
            self.load_stats.decode_ms += decode_ms;
            self.load_stats.mip_ms += mip_ms;
            self.load_stats.decoded_images += 1;
            return Ok(image);
        }

        // A material may intentionally reference a shared/global asset. Preserve
        // that legal Rend2/JKA behavior when its own provider does not carry it.
        self.decode(assets, path, clamp, srgb)
    }

    fn decode(
        &mut self,
        assets: &mut AssetSearchPath,
        path: &str,
        clamp: bool,
        srgb: bool,
    ) -> Result<TextureData, String> {
        let stem = path
            .rsplit_once('.')
            .filter(|(_, ext)| ["tga", "jpg", "jpeg", "png"].contains(ext))
            .map(|(stem, _)| stem)
            .unwrap_or(path);
        let mut candidates = vec![path.to_string()];
        for ext in ["tga", "jpg", "png"] {
            let candidate = format!("{stem}.{ext}");
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }

        if !assets.asset_overrides_allowed() {
            for name in &candidates {
                let read_started = Instant::now();
                let asset = assets
                    .read_stock(name, 64 * 1024 * 1024)
                    .map_err(|e| e.to_string())?;
                self.load_stats.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
                let Some(asset) = asset else {
                    continue;
                };
                let (mut image, decode_ms, mip_ms) =
                    decode_texture_data_profiled(name, &asset.bytes, clamp, true, srgb)?;
                image.source = Some(asset.source);
                self.load_stats.decode_ms += decode_ms;
                self.load_stats.mip_ms += mip_ms;
                self.load_stats.decoded_images += 1;
                return Ok(image);
            }
        }

        for name in &candidates {
            let read_started = Instant::now();
            let asset = assets
                .read(name, 64 * 1024 * 1024)
                .map_err(|e| e.to_string())?;
            self.load_stats.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
            let Some(asset) = asset else {
                continue;
            };
            let (mut image, decode_ms, mip_ms) =
                decode_texture_data_profiled(name, &asset.bytes, clamp, true, srgb)?;
            image.source = Some(asset.source);
            self.load_stats.decode_ms += decode_ms;
            self.load_stats.mip_ms += mip_ms;
            self.load_stats.decoded_images += 1;
            return Ok(image);
        }
        Err(format!("Missing texture: {path}"))
    }
}

pub fn decode_texture_data(
    name: &str,
    bytes: &[u8],
    clamp: bool,
    mipmaps: bool,
) -> Result<TextureData, String> {
    decode_texture_data_with_color_space(name, bytes, clamp, mipmaps, true)
}

pub fn decode_texture_data_with_color_space(
    name: &str,
    bytes: &[u8],
    clamp: bool,
    mipmaps: bool,
    srgb: bool,
) -> Result<TextureData, String> {
    decode_texture_data_profiled(name, bytes, clamp, mipmaps, srgb).map(|(image, _, _)| image)
}

pub fn decode_hdr_texture_data(
    name: &str,
    bytes: &[u8],
    clamp: bool,
    linear_scale: f32,
) -> Result<TextureData, String> {
    let image = image::load_from_memory_with_format(bytes, ImageFormat::Hdr)
        .map_err(|e| format!("{name}: {e}"))?
        .to_rgb32f();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(format!("{name}: texture dimensions outside 1..8192"));
    }
    let rgb = image.into_raw();
    let mut rgba16f = Vec::with_capacity(width as usize * height as usize * 4);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in rgb.chunks_exact(3) {
        let linear = [
            pixel[0].max(0.0) * linear_scale,
            pixel[1].max(0.0) * linear_scale,
            pixel[2].max(0.0) * linear_scale,
        ];
        rgba16f.extend_from_slice(&[
            f32_to_f16_bits(linear[0]),
            f32_to_f16_bits(linear[1]),
            f32_to_f16_bits(linear[2]),
            f32_to_f16_bits(1.0),
        ]);
        // Keep a clipped sRGB inspection/AO copy. Rendering uses rgba16f.
        for channel in linear {
            let encoded = if channel <= 0.003_130_8 {
                channel * 12.92
            } else {
                1.055 * channel.powf(1.0 / 2.4) - 0.055
            };
            rgba.push((encoded.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        rgba.push(255);
    }
    Ok(TextureData {
        label: name.to_owned(),
        source: None,
        width,
        height,
        rgba,
        rgba16f: Some(rgba16f),
        mip_level_count: 1,
        clamp,
        srgb: false,
    })
}

/// Rend2-compatible LDR -> FP16 lightmap promotion used when
/// r_floatLightmap is enabled but no .hdr companion exists. This mirrors the
/// rd-rend2 path: operate on lightmap bytes as linear intensity, grey very dark
/// texels to avoid coloured splotches, then normalize by 255.
pub fn promote_lightmap_to_float(image: &mut TextureData) {
    if image.rgba16f.is_some() {
        image.srgb = false;
        return;
    }
    let base_len = image.width as usize * image.height as usize * 4;
    if image.rgba.len() < base_len {
        return;
    }
    let mut rgba16f = Vec::with_capacity(base_len);
    for pixel in image.rgba[..base_len].chunks_exact(4) {
        let mut rgb = [f32::from(pixel[0]), f32::from(pixel[1]), f32::from(pixel[2])];
        if rgb[0] + rgb[1] + rgb[2] < 12.0 {
            let average = (rgb[0] + rgb[1] + rgb[2]) * (1.0 / 3.0);
            rgb = [average; 3];
        }
        rgba16f.extend_from_slice(&[
            f32_to_f16_bits(rgb[0] / 255.0),
            f32_to_f16_bits(rgb[1] / 255.0),
            f32_to_f16_bits(rgb[2] / 255.0),
            f32_to_f16_bits(1.0),
        ]);
    }
    image.rgba16f = Some(rgba16f);
    image.srgb = false;
    image.mip_level_count = 1;
}

fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ff_ff;
    if exponent == 0xff {
        return sign | if mantissa == 0 { 0x7c00 } else { 0x7e00 };
    }
    let half_exp = exponent - 127 + 15;
    if half_exp >= 31 {
        return sign | 0x7bff;
    }
    if half_exp <= 0 {
        if half_exp < -10 {
            return sign;
        }
        let mantissa = mantissa | 0x80_00_00;
        let shift = (14 - half_exp) as u32;
        let mut half = (mantissa >> shift) as u16;
        if ((mantissa >> (shift - 1)) & 1) != 0 {
            half = half.saturating_add(1);
        }
        return sign | half;
    }
    let mut half = sign | ((half_exp as u16) << 10) | ((mantissa >> 13) as u16);
    if (mantissa & 0x1000) != 0 {
        half = half.saturating_add(1);
    }
    half
}

fn decode_texture_data_profiled(
    name: &str,
    bytes: &[u8],
    clamp: bool,
    mipmaps: bool,
    srgb: bool,
) -> Result<(TextureData, f64, f64), String> {
    let format = match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "tga" => ImageFormat::Tga,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "png" => ImageFormat::Png,
        ext => return Err(format!("{name}: unsupported image extension {ext}")),
    };
    let decode_started = Instant::now();
    let image = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| format!("{name}: {e}"))?
        .to_rgba8();
    let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(format!("{name}: texture dimensions outside 1..8192"));
    }
    let base = image.into_raw();
    let mip_started = Instant::now();
    let (rgba, mip_level_count) = if mipmaps {
        build_mip_chain(width, height, base)
    } else {
        (base, 1)
    };
    let mip_ms = mip_started.elapsed().as_secs_f64() * 1000.0;
    Ok((
        TextureData {
            label: name.to_owned(),
            source: None,
            width,
            height,
            rgba,
            rgba16f: None,
            mip_level_count,
            clamp,
            srgb,
        },
        decode_ms,
        mip_ms,
    ))
}

fn build_normal_height_mip_chain(width: u32, height: u32, base: Vec<u8>) -> (Vec<u8>, u32) {
    let decode = |value: u8| f32::from(value) * (2.0 / 255.0) - 1.0;
    let encode = |value: f32| -> u8 {
        ((value * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
    };
    let mut all = base.clone();
    let mut previous = base;
    let mut previous_width = width;
    let mut previous_height = height;
    let mut levels = 1_u32;

    while previous_width > 1 || previous_height > 1 {
        let width = (previous_width / 2).max(1);
        let height = (previous_height / 2).max(1);
        let mut next = vec![0_u8; width as usize * height as usize * 4];
        for y in 0..height {
            for x in 0..width {
                let mut normal = [0.0_f32; 3];
                let mut max_height = 0_u8;
                for oy in 0..2 {
                    for ox in 0..2 {
                        let sx = (x * 2 + ox).min(previous_width - 1);
                        let sy = (y * 2 + oy).min(previous_height - 1);
                        let index = ((sy * previous_width + sx) * 4) as usize;
                        normal[0] += decode(previous[index]);
                        normal[1] += decode(previous[index + 1]);
                        normal[2] += decode(previous[index + 2]);
                        max_height = max_height.max(previous[index + 3]);
                    }
                }
                let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
                if length > 1.0e-8 {
                    normal = normal.map(|component| component / length);
                } else {
                    normal = [0.0, 0.0, 1.0];
                }
                let index = ((y * width + x) * 4) as usize;
                next[index] = encode(normal[0]);
                next[index + 1] = encode(normal[1]);
                next[index + 2] = encode(normal[2]);
                // Rend2's R_MipMapNormalHeight preserves the strongest height
                // sample instead of averaging it away.
                next[index + 3] = max_height;
            }
        }
        all.extend_from_slice(&next);
        previous = next;
        previous_width = width;
        previous_height = height;
        levels += 1;
    }

    (all, levels)
}

fn build_mip_chain(width: u32, height: u32, base: Vec<u8>) -> (Vec<u8>, u32) {
    let mut all = base.clone();
    let mut previous = base;
    let mut previous_width = width;
    let mut previous_height = height;
    let mut levels = 1_u32;

    while previous_width > 1 || previous_height > 1 {
        let width = (previous_width / 2).max(1);
        let height = (previous_height / 2).max(1);
        let mut next = vec![0_u8; width as usize * height as usize * 4];
        for y in 0..height {
            for x in 0..width {
                for channel in 0..4_usize {
                    let mut sum = 0_u32;
                    let mut samples = 0_u32;
                    for oy in 0..2 {
                        for ox in 0..2 {
                            let sx = (x * 2 + ox).min(previous_width - 1);
                            let sy = (y * 2 + oy).min(previous_height - 1);
                            let index = ((sy * previous_width + sx) * 4) as usize + channel;
                            sum += u32::from(previous[index]);
                            samples += 1;
                        }
                    }
                    let index = ((y * width + x) * 4) as usize + channel;
                    next[index] = (sum / samples) as u8;
                }
            }
        }
        all.extend_from_slice(&next);
        previous = next;
        previous_width = width;
        previous_height = height;
        levels += 1;
    }

    (all, levels)
}

fn companion_stem(label: &str) -> &str {
    label
        .rsplit_once('.')
        .filter(|(_, ext)| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "tga" | "jpg" | "jpeg" | "png"
            )
        })
        .map(|(stem, _)| stem)
        .unwrap_or(label)
}

fn optional_companion(
    assets: &mut AssetSearchPath,
    textures: &mut Textures,
    base_index: usize,
    suffixes: &[&str],
    preferred_source: Option<&Path>,
) -> Option<usize> {
    let (base, clamp) = {
        let image = textures.images.get(base_index)?;
        (image.label.clone(), image.clamp)
    };
    let stem = companion_stem(&base);
    suffixes.iter().find_map(|suffix| {
        let path = format!("{stem}{suffix}");
        preferred_source
            .and_then(|source| textures.load_optional_linear_from_source(assets, &path, clamp, source))
            .or_else(|| textures.load_optional_linear(assets, &path, clamp))
    })
}

fn optional_companion_srgb(
    assets: &mut AssetSearchPath,
    textures: &mut Textures,
    base_index: usize,
    suffixes: &[&str],
    preferred_source: Option<&Path>,
) -> Option<usize> {
    let (base, clamp) = {
        let image = textures.images.get(base_index)?;
        (image.label.clone(), image.clamp)
    };
    let stem = companion_stem(&base);
    suffixes.iter().find_map(|suffix| {
        let path = format!("{stem}{suffix}");
        preferred_source
            .and_then(|source| textures.load_optional_srgb_from_source(assets, &path, clamp, source))
            .or_else(|| textures.load_optional_srgb(assets, &path, clamp))
    })
}

fn blend_factor(value: &str, source: bool) -> Option<BlendFactor> {
    match value {
        "gl_zero" => Some(BlendFactor::Zero),
        "gl_one" => Some(BlendFactor::One),
        "gl_src_color" if !source => Some(BlendFactor::SrcColor),
        "gl_one_minus_src_color" if !source => Some(BlendFactor::OneMinusSrcColor),
        "gl_src_alpha" => Some(BlendFactor::SrcAlpha),
        "gl_one_minus_src_alpha" => Some(BlendFactor::OneMinusSrcAlpha),
        "gl_dst_color" if source => Some(BlendFactor::DstColor),
        "gl_one_minus_dst_color" if source => Some(BlendFactor::OneMinusDstColor),
        "gl_dst_alpha" => Some(BlendFactor::DstAlpha),
        "gl_one_minus_dst_alpha" => Some(BlendFactor::OneMinusDstAlpha),
        "gl_src_alpha_saturate" if source => Some(BlendFactor::SrcAlphaSaturate),
        _ => None,
    }
}

fn stage_blend(stage: &shader::Stage) -> Option<BlendFunc> {
    match stage.blend.as_str() {
        "" => None,
        "blend" => Some(BlendFunc {
            src: BlendFactor::SrcAlpha,
            dst: BlendFactor::OneMinusSrcAlpha,
        }),
        "add" => Some(BlendFunc {
            src: BlendFactor::One,
            dst: BlendFactor::One,
        }),
        "filter" => Some(BlendFunc {
            src: BlendFactor::DstColor,
            dst: BlendFactor::Zero,
        }),
        value => {
            let mut words = value.split_whitespace();
            let src = blend_factor(words.next()?, true)?;
            let dst = blend_factor(words.next()?, false)?;
            if words.next().is_some() {
                return None;
            }
            Some(BlendFunc { src, dst })
        }
    }
}

fn alpha_cutoff(stage: &shader::Stage) -> f32 {
    match stage.alpha_test.as_str() {
        "ge128" => 0.5,
        "gt0" => 1.0 / 255.0,
        _ => 0.0,
    }
}

fn cull_mode(value: &str) -> CullMode {
    // world_mesh uses OpenJK winding; scene preparation reverses triangles to CCW.
    match value {
        "none" | "twosided" | "disable" => CullMode::None,
        "back" | "backsided" => CullMode::Front,
        _ => CullMode::Back,
    }
}

fn stage_enhancements(
    source: Option<&shader::Stage>,
    base_index: Option<usize>,
    assets: &mut AssetSearchPath,
    textures: &mut Textures,
    gen_normal_maps: bool,
    preferred_source: Option<&Path>,
) -> StageEnhancements {
    let explicit_linear =
        |path: Option<&String>, assets: &mut AssetSearchPath, textures: &mut Textures| {
            path.and_then(|path| {
                preferred_source
                    .and_then(|source| {
                        textures.load_optional_linear_from_source(assets, path, false, source)
                    })
                    .or_else(|| textures.load_optional_linear(assets, path, false))
            })
        };

    let explicit_normal_height = source
        .and_then(|stage| explicit_linear(stage.normal_height_map.as_ref(), assets, textures));
    let autoload_normal_height = if explicit_normal_height.is_none() {
        base_index.and_then(|index| optional_companion(assets, textures, index, &["_nh"], preferred_source))
    } else {
        None
    };
    let normal_height = explicit_normal_height.or(autoload_normal_height);

    let normal_texture = normal_height
        .or_else(|| {
            source
                .and_then(|stage| explicit_linear(stage.normal_map.as_ref(), assets, textures))
                .or_else(|| {
                    base_index.and_then(|index| {
                        optional_companion(assets, textures, index, &["_normal", "_norm", "_n"], preferred_source)
                    })
                })
        })
        .or_else(|| {
            (gen_normal_maps).then(|| base_index.and_then(|index| textures.generated_normal(index))).flatten()
        });

    let explicit_rmo =
        source.and_then(|stage| explicit_linear(stage.rmo_map.as_ref(), assets, textures));
    let autoload_rmo = if explicit_rmo.is_none() {
        base_index.and_then(|index| optional_companion(assets, textures, index, &["_rmo"], preferred_source))
    } else {
        None
    };
    let rmo = explicit_rmo.or(autoload_rmo);

    let roughness_texture = rmo.or_else(|| {
        base_index.and_then(|index| {
            optional_companion(assets, textures, index, &["_roughness", "_rough", "_r"], preferred_source)
        })
    });
    let metallic_texture = rmo.or_else(|| {
        base_index.and_then(|index| {
            optional_companion(
                assets,
                textures,
                index,
                &["_metallic", "_metalness", "_metal"],
                preferred_source,
            )
        })
    });
    let height_texture = normal_height.or_else(|| {
        base_index.and_then(|index| {
            optional_companion(
                assets,
                textures,
                index,
                &["_height", "_disp", "_displacement", "_h"],
                preferred_source,
            )
        })
    });
    let specular_texture = source
        .and_then(|stage| explicit_linear(stage.specular_map.as_ref(), assets, textures))
        .or_else(|| {
            base_index.and_then(|index| {
                optional_companion(assets, textures, index, &["_specular", "_spec"], preferred_source)
            })
        });
    let emissive_texture = base_index.and_then(|index| {
        optional_companion_srgb(
            assets,
            textures,
            index,
            &["_emissive", "_emission", "_emit", "_glow"],
            preferred_source,
        )
    });

    StageEnhancements {
        normal_texture,
        roughness_texture,
        height_texture,
        metallic_texture,
        specular_texture,
        emissive_texture,
        height_from_alpha: normal_height.is_some(),
        rmo_packed: rmo.is_some(),
        rmo_specular_alpha: source.is_some_and(|stage| stage.rmo_specular_alpha),
        normal_scale: source
            .and_then(|stage| stage.normal_scale)
            .unwrap_or([1.0, 1.0]),
        roughness_override: source.and_then(|stage| stage.roughness),
        specular_reflectance: source.and_then(|stage| stage.specular_reflectance),
        parallax_depth: source.and_then(|stage| stage.parallax_depth).unwrap_or(0.0),
    }
}

fn normalized_texture_average(texture: &TextureData) -> [f32; 3] {
    let pixel_count = usize::try_from(texture.width)
        .ok()
        .and_then(|width| {
            usize::try_from(texture.height)
                .ok()
                .map(|height| width * height)
        })
        .unwrap_or(0);
    let base_bytes = pixel_count.saturating_mul(4).min(texture.rgba.len());
    if base_bytes < 4 {
        return [1.0; 3];
    }
    let mut sum = [0.0_f64; 3];
    let mut count = 0_u64;
    for pixel in texture.rgba[..base_bytes].chunks_exact(4) {
        // q3map_lightimage is used for color/chromaticity, so alpha is not
        // part of the average. Decode sRGB before averaging because the GPU
        // samples ordinary color textures in linear space as well.
        for channel in 0..3 {
            let srgb = f64::from(pixel[channel]) / 255.0;
            sum[channel] += if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            };
        }
        count += 1;
    }
    if count == 0 {
        return [1.0; 3];
    }
    let average = sum.map(|value| (value / count as f64) as f32);
    let length =
        (average[0] * average[0] + average[1] * average[1] + average[2] * average[2]).sqrt();
    if length <= 1e-6 {
        [1.0; 3]
    } else {
        average.map(|value| value / length)
    }
}

fn emissive_surface_light_stats(texture: &TextureData) -> Option<([f32; 3], f32)> {
    let pixel_count = usize::try_from(texture.width)
        .ok()
        .and_then(|width| {
            usize::try_from(texture.height)
                .ok()
                .map(|height| width.saturating_mul(height))
        })
        .unwrap_or(0);
    let base_bytes = pixel_count.saturating_mul(4).min(texture.rgba.len());
    if base_bytes < 4 || pixel_count == 0 {
        return None;
    }

    let mut weighted_rgb = [0.0_f64; 3];
    let mut luminance_sum = 0.0_f64;
    let mut active = 0_u64;
    for pixel in texture.rgba[..base_bytes].chunks_exact(4) {
        let rgb = [pixel[0], pixel[1], pixel[2]].map(|channel| {
            let srgb = f64::from(channel) / 255.0;
            if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            }
        });
        let luminance = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
        if luminance > 0.01 {
            active += 1;
        }
        luminance_sum += luminance;
        for channel in 0..3 {
            weighted_rgb[channel] += rgb[channel] * luminance.max(1e-6);
        }
    }

    let mean_luminance = luminance_sum / pixel_count as f64;
    let coverage = active as f64 / pixel_count as f64;
    if mean_luminance < 0.002 || coverage < 0.001 {
        return None;
    }

    let length = (weighted_rgb[0] * weighted_rgb[0]
        + weighted_rgb[1] * weighted_rgb[1]
        + weighted_rgb[2] * weighted_rgb[2])
        .sqrt();
    let color = if length > 1e-9 {
        weighted_rgb.map(|value| (value / length) as f32)
    } else {
        [1.0; 3]
    };

    // Keep inferred emitters intentionally below the strongest authored q3map
    // lights. Coverage damps tiny glow masks while sqrt(mean) preserves useful
    // output from soft emissive panels.
    let coverage_scale = coverage.sqrt().clamp(0.2, 1.0);
    let value =
        ((220.0 + 680.0 * mean_luminance.sqrt()) * coverage_scale).clamp(64.0, 800.0) as f32;
    Some((color, value))
}

pub fn primary_texture_requests(
    used_shader_names: &[String],
    library: &BTreeMap<String, Shader>,
) -> Vec<(String, bool, bool)> {
    let mut requests = Vec::<(String, bool, bool)>::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut push = |path: &str, clamp: bool, srgb: bool| {
        if path.is_empty() || path.starts_with('$') {
            return;
        }
        let key = (path.replace('\\', "/").to_ascii_lowercase(), clamp, srgb);
        if seen.insert(key.clone()) {
            requests.push(key);
        }
    };

    for name in used_shader_names {
        match library.get(name) {
            Some(shader) if !shader.stages.is_empty() || shader.sky => {
                if let Some(path) = shader.light_image.as_deref() {
                    push(path, false, true);
                }
                if let Some(prefix) = shader.sky_box.as_deref() {
                    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
                        push(&format!("{prefix}_{suffix}"), true, true);
                    }
                }
                for stage in &shader.stages {
                    if stage.surface_sprite.is_some_and(|sprite| {
                        matches!(sprite.kind, shader::SurfaceSpriteType::Vertical)
                    }) {
                        continue;
                    }
                    push(&stage.image, stage.clamp, true);
                    if let Some(path) = stage.normal_map.as_deref() {
                        push(path, false, false);
                    }
                    if let Some(path) = stage.normal_height_map.as_deref() {
                        push(path, false, false);
                    }
                    if let Some(path) = stage.rmo_map.as_deref() {
                        push(path, false, false);
                    }
                    if let Some(path) = stage.specular_map.as_deref() {
                        push(path, false, false);
                    }
                }
            }
            _ => push(name, false, true),
        }
    }
    requests
}

pub fn describe(
    name: &str,
    flags: u32,
    definition: Option<&Shader>,
    origin: Option<&ShaderDefinitionOrigin>,
    assets: &mut AssetSearchPath,
    textures: &mut Textures,
    gen_normal_maps: bool,
) -> SurfaceMaterial {
    let empty = Shader::default();
    let definition = definition.unwrap_or(&empty);
    let explicit = !definition.stages.is_empty();
    let preferred_source = origin
        .filter(|origin| origin.mtr_override)
        .map(|origin| origin.source.as_path());
    let sky = flags & SURF_SKY != 0 || definition.sky;
    let base_hidden = definition.nodraw || flags & SURF_NODRAW != 0;
    let surface_sprite_cull_quirk = definition
        .stages
        .iter()
        .any(|stage| stage.surface_sprite.is_some());
    let mut surface_light = (definition.surface_light > 0.0).then(|| {
        // q3map uses q3map_lightimage for emitter color when present; without
        // one, its shader info falls back to the surface image's average color.
        let color_image = definition.light_image.as_deref().or_else(|| {
            definition
                .stages
                .iter()
                .map(|stage| stage.image.as_str())
                .find(|image| !image.is_empty() && !image.starts_with('$'))
        });
        let color = color_image
            .and_then(|path| {
                preferred_source
                    .and_then(|source| textures.load_from_source(assets, path, false, source))
                    .or_else(|| textures.load(assets, path, false))
            })
            .and_then(|index| textures.images.get(index))
            .map(normalized_texture_average)
            .unwrap_or([1.0; 3]);
        SurfaceLight {
            value: definition.surface_light,
            color,
            // q3map2's defaultLightSubdivide is 120 map units; preserve an
            // explicit authored override when one exists.
            subdivide: definition
                .light_subdivide
                .unwrap_or(120.0)
                .clamp(16.0, 1024.0),
            inferred_from_emissive: false,
        }
    });
    let skybox = if sky {
        definition
            .sky_box
            .as_deref()
            .and_then(|prefix| {
                preferred_source
                    .and_then(|source| textures.load_skybox_from_source(assets, prefix, source))
                    .or_else(|| textures.load_skybox(assets, prefix))
            })
    } else {
        None
    };
    let mut stages = Vec::new();
    let mut grass = None;
    let mut surface_sprite_effects = Vec::new();

    if !sky && !base_hidden {
        if explicit {
            for source in &definition.stages {
                if let Some(sprite) = source.surface_sprite {
                    if matches!(sprite.kind, shader::SurfaceSpriteType::Vertical) {
                        if grass.is_none() {
                            grass = Some(GrassMaterial {
                                authored_width: sprite.width,
                                height: sprite.height,
                                density: sprite.density,
                                fade_dist: sprite.fade_dist,
                                fade_max: sprite.fade_max.max(sprite.fade_dist + 1.0),
                                wind: sprite.wind,
                                any_angle: matches!(
                                    sprite.facing,
                                    shader::SurfaceSpriteFacing::AnyAngle
                                ),
                            });
                        } else {
                            textures.warnings.push(format!(
                                "{name}: multiple vertical surfaceSprites stages; using the first emitter"
                            ));
                        }
                        // Vertical surfaceSprites is now procedural grass metadata,
                        // never an ordinary alpha-card material pass.
                        continue;
                    }
                    if matches!(sprite.kind, shader::SurfaceSpriteType::Effect) {
                        if source.image.is_empty() || source.image.starts_with('$') {
                            textures.warnings.push(format!(
                                "{name}: surfaceSprites effect has no ordinary image; skipping emitter"
                            ));
                            continue;
                        }
                        let Some(texture) = textures.load(assets, &source.image, source.clamp) else {
                            textures.warnings.push(format!(
                                "{name}: surfaceSprites effect texture {} could not be loaded",
                                source.image
                            ));
                            continue;
                        };
                        let blend = stage_blend(source);
                        if !source.blend.is_empty() && blend.is_none() {
                            textures.warnings.push(format!(
                                "{name}: unsupported surfaceSprites effect blendFunc {}; using opaque blend",
                                source.blend
                            ));
                        }
                        surface_sprite_effects.push(SurfaceSpriteEffectMaterial {
                            texture,
                            clamp: source.clamp,
                            blend,
                            sprite,
                        });
                        // Effect surfaceSprites are procedural QuickSprite quads. Rendering
                        // the clampmap as a normal BSP stage stretches one splash over the
                        // entire parent polygon, which is exactly the bug this path avoids.
                        continue;
                    }
                    textures.warnings.push(format!(
                        "{name}: surfaceSprites {:?} is not handled procedurally; preserving the existing material-stage fallback",
                        sprite.kind
                    ));
                }
                if source.image.is_empty() {
                    continue;
                }
                let texture = match source.image.as_str() {
                    "$lightmap" => StageTexture::Lightmap,
                    "$whiteimage" | "*white" => StageTexture::White,
                    value if value.starts_with('$') => {
                        textures.warnings.push(format!(
                            "{name}: unsupported generated image {value}; using white"
                        ));
                        StageTexture::White
                    }
                    value => preferred_source
                        .and_then(|provider| {
                            textures.load_from_source(assets, value, source.clamp, provider)
                        })
                        .or_else(|| textures.load(assets, value, source.clamp))
                        .map(StageTexture::Image)
                        .unwrap_or(StageTexture::White),
                };
                let tc_gen = if matches!(texture, StageTexture::Lightmap)
                    && matches!(source.tc_gen, TcGen::Base)
                {
                    TcGen::Lightmap
                } else {
                    source.tc_gen
                };
                let blend = stage_blend(source);
                if !source.blend.is_empty() && blend.is_none() {
                    textures.warnings.push(format!(
                        "{name}: unsupported blendFunc {}; rendering stage opaque",
                        source.blend
                    ));
                }
                let base_index = match texture {
                    StageTexture::Image(index) => Some(index),
                    StageTexture::Lightmap | StageTexture::White => None,
                };
                let enhancements = stage_enhancements(
                    Some(source),
                    base_index,
                    assets,
                    textures,
                    gen_normal_maps,
                    preferred_source,
                );
                stages.push(MaterialStage {
                    texture,
                    enhancements,
                    blend,
                    alpha_cutoff: alpha_cutoff(source),
                    opacity: source.alpha.unwrap_or(1.0),
                    color: source.color.unwrap_or([1.0; 3]),
                    rgb_gen: source.rgb_gen,
                    alpha_gen: source.alpha_gen,
                    tc_gen,
                    tc_mods: source.tc_mods.clone(),
                    depth_write: source.depth_write,
                    depth_equal: source.depth_equal,
                });
            }
        } else {
            let texture = textures
                .load(assets, name, false)
                .map(StageTexture::Image)
                .unwrap_or(StageTexture::White);
            let base_index = match texture {
                StageTexture::Image(index) => Some(index),
                StageTexture::Lightmap | StageTexture::White => None,
            };
            let enhancements = stage_enhancements(
                None,
                base_index,
                assets,
                textures,
                gen_normal_maps,
                None,
            );
            stages.push(MaterialStage {
                texture,
                enhancements,
                blend: None,
                alpha_cutoff: 0.0,
                opacity: 1.0,
                color: [1.0; 3],
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
                tc_gen: TcGen::Base,
                tc_mods: Vec::new(),
                depth_write: false,
                depth_equal: false,
            });
        }
    }

    // Modern material packs often express glow only through an emissive
    // companion map and have no q3map_surfacelight directive. Promote the first
    // meaningful emissive companion to an opt-in runtime area emitter. The
    // conservative strength estimate is based on linear-space texture energy so
    // a mostly-black mask does not light the whole map like a full-white panel.
    if surface_light.is_none() {
        let emissive_texture = stages
            .iter()
            .find_map(|stage| stage.enhancements.emissive_texture);
        if let Some((color, value)) = emissive_texture
            .and_then(|index| textures.images.get(index))
            .and_then(emissive_surface_light_stats)
        {
            surface_light = Some(SurfaceLight {
                value,
                color,
                subdivide: 120.0,
                inferred_from_emissive: true,
            });
        }
    }

    for unsupported in definition.unsupported.iter().chain(
        definition
            .stages
            .iter()
            .flat_map(|stage| &stage.unsupported),
    ) {
        textures
            .warnings
            .push(format!("{name}: approximated/unsupported {unsupported}"));
    }

    SurfaceMaterial {
        surface_material: (flags & MATERIAL_MASK) as u8,
        hidden: !sky
            && (base_hidden
                || (explicit && stages.is_empty() && surface_sprite_effects.is_empty())),
        surface_light,
        grass,
        surface_sprite_effects,
        surface_sprite_cull_quirk,
        stages,
        cull: cull_mode(&definition.cull),
        offset: definition.polygon_offset,
        planar_reflection: definition.portal,
        water: definition.water,
        alpha_shadow: definition.alpha_shadow,
        light_filter: definition.light_filter,
        no_fog: definition.no_fog,
        sky,
        skybox,
        explicit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(text: &str) -> SurfaceMaterial {
        let shaders = shader::parse(text).unwrap();
        let root =
            std::env::temp_dir().join(format!("jka-material-fixture-{}", std::process::id()));
        let mut assets = AssetSearchPath::open(&root).unwrap();
        describe(
            "test",
            0,
            shaders.get("test"),
            None,
            &mut assets,
            &mut Textures::new(),
            false,
        )
    }

    #[test]
    fn emissive_texture_stats_reject_black_and_extract_color() {
        let black = TextureData {
            label: "black".into(),
            source: None,
            width: 2,
            height: 2,
            rgba: [0_u8, 0, 0, 255].repeat(4),
            rgba16f: None,
            mip_level_count: 1,
            clamp: false,
            srgb: true,
        };
        assert!(emissive_surface_light_stats(&black).is_none());

        let red = TextureData {
            label: "red".into(),
            source: None,
            width: 2,
            height: 2,
            rgba: [255_u8, 32, 16, 255].repeat(4),
            rgba16f: None,
            mip_level_count: 1,
            clamp: false,
            srgb: true,
        };
        let (color, value) = emissive_surface_light_stats(&red).expect("emissive stats");
        assert!(color[0] > color[1] && color[1] > color[2]);
        assert!((64.0..=800.0).contains(&value));
    }

    #[test]
    fn sky_without_diffuse_remains_visible_to_depth_pass() {
        let sky = material(
            "test {\nsurfaceparm sky\nsurfaceparm nodraw\nskyparms skies/example 512 -\n}",
        );
        assert!(sky.sky);
    }

    #[test]
    fn ordered_shader_stages_are_preserved() {
        let material = material(
            "test {\n{\nmap textures/factory/enviro\ntcGen environment\n}\n{\nmap textures/imperial/square\nblendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA\n}\n{\nmap $lightmap\nblendFunc GL_DST_COLOR GL_ZERO\n}\n}",
        );
        assert!(material.explicit);
        assert_eq!(material.stages.len(), 3);
        assert!(matches!(material.stages[0].tc_gen, TcGen::Environment));
        assert_eq!(
            material.stages[1].blend,
            Some(BlendFunc {
                src: BlendFactor::SrcAlpha,
                dst: BlendFactor::OneMinusSrcAlpha,
            })
        );
        assert_eq!(
            material.stages[2].blend,
            Some(BlendFunc {
                src: BlendFactor::DstColor,
                dst: BlendFactor::Zero,
            })
        );
        assert_eq!(material.stages[2].texture, StageTexture::Lightmap);
    }
    #[test]
    fn detail_blend_keeps_destination_color() {
        let material = material(
            "test {\n{\nmap $lightmap\n}\n{\nmap textures/desert/stucco_grime_top_bottom\nblendFunc GL_DST_COLOR GL_ZERO\n}\n{\nmap textures/common/detail5\nblendFunc GL_DST_COLOR GL_ONE\nrgbGen identity\ntcMod scale 3 3\n}\n}",
        );
        assert_eq!(material.stages.len(), 3);
        assert_eq!(
            material.stages[2].blend,
            Some(BlendFunc {
                src: BlendFactor::DstColor,
                dst: BlendFactor::One,
            })
        );
        assert!(matches!(
            material.stages[2].tc_mods.as_slice(),
            [TcMod::Scale(3.0, 3.0)]
        ));
    }
    #[test]
    fn surface_sprite_stage_becomes_grass_metadata_not_a_material_pass() {
        let material = material(
            "test {\n{\nmap textures/test/ground\n}\n{\nmap gfx/sprites/y_grass_tall\nsurfaceSprites vertical 32 36 42 500\nssFadeMax 1500\nssVariance 1 2\nssWind 0.5\nalphaFunc GE192\nblendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA\nrgbGen vertex\n}\n}",
        );
        assert_eq!(material.stages.len(), 1);
        let grass = material.grass.expect("grass metadata");
        assert_eq!(grass.height, 36.0);
        assert_eq!(grass.density, 42.0);
        assert_eq!(grass.fade_max, 1500.0);
        assert_eq!(grass.wind, 0.5);
        assert!(material.surface_sprite_cull_quirk);
    }

    #[test]
    fn effect_surface_sprite_is_not_rendered_as_a_stretched_material_stage() {
        let material = material(
            "test {
{
map textures/test/road
}
{
clampmap textures/test/rain
surfaceSprites effect 1.5 1.5 64 512
ssFXDuration 135
ssFXGrow 6 6
ssFXAlphaRange 0.30 0
ssFadeMax 768
blendFunc GL_ONE GL_ONE
}
}",
        );
        assert_eq!(material.stages.len(), 1);
        assert_eq!(material.surface_sprite_effects.len(), 1);
        let effect = material.surface_sprite_effects[0];
        assert_eq!(effect.sprite.kind, shader::SurfaceSpriteType::Effect);
        assert_eq!(effect.sprite.fx_duration, 135.0);
        assert_eq!(effect.sprite.fx_grow, [6.0, 6.0]);
        assert_eq!(
            effect.blend,
            Some(BlendFunc {
                src: BlendFactor::One,
                dst: BlendFactor::One,
            })
        );
        assert!(effect.clamp);
    }
}
