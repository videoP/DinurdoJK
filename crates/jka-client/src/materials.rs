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
use std::{collections::BTreeMap, path::{Path, PathBuf}, sync::Arc, time::Instant};

/// Largest texture file read from disk/pk3. Matches the 8192x8192 RGBA decode
/// cap in `decode_texture_data_profiled` plus header/footer slack (a 4096^2 TGA
/// sky is already 64 MiB + 44 bytes).
const MAX_TEXTURE_FILE_BYTES: usize = 8192 * 8192 * 4 + 1024;
const MAX_VIDEO_FILE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlendFunc {
    pub src: BlendFactor,
    pub dst: BlendFactor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    /// `skyParms` cloud height for a sky shader that authored one; zero otherwise.
    /// A sky's `stages` are its cloud layers: each is drawn over the outer box
    /// with its coordinates generated from the view direction (`TcGen::SkyCloud`).
    pub sky_cloud_height: f32,
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
    /// At least one authored shader stage carries the classic id Tech 3
    /// `detail` marker. The enhanced fallback detail system must leave these
    /// materials alone rather than layering a second micro-detail treatment.
    pub authored_detail: bool,
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
            sky_cloud_height: 0.0,
            hidden: false,
            surface_light: None,
            grass: None,
            surface_sprite_effects: Vec::new(),
            surface_sprite_cull_quirk: false,
            explicit: false,
            authored_detail: false,
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
    /// Preloaded images and decode time (summed over workers) by file type:
    /// tga, jpg, png.
    pub format_images: [u32; 3],
    pub format_decode_ms: [f64; 3],
    /// Serial time spent in `Textures::generated_normal` (Sobel + mip chain).
    pub generated_normal_ms: f64,
    pub generated_normals: usize,
}

/// Readers lent to texture preload jobs. A job takes an idle reader (or forks a
/// new one) so concurrent jobs never share an archive handle, then returns it.
struct TextureReaders {
    idle: std::sync::Mutex<Vec<AssetSearchPath>>,
    template: std::sync::Mutex<AssetSearchPath>,
    overrides_allowed: bool,
}

impl TextureReaders {
    /// First readable candidate, with the same stock-before-override order the
    /// loader thread used to apply: when overrides are disabled, retail
    /// providers are tried for every candidate before ordinary reads.
    fn read_first(
        &self,
        candidates: &[String],
    ) -> Result<Option<(String, PathBuf, Vec<u8>)>, String> {
        let lent = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop();
        let mut assets = lent.unwrap_or_else(|| {
            self.template
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fork()
        });
        let result = (|| {
            if !self.overrides_allowed {
                for name in candidates {
                    let asset = assets
                        .read_stock(name, MAX_TEXTURE_FILE_BYTES)
                        .map_err(|e| e.to_string())?;
                    if let Some(asset) = asset {
                        return Ok(Some((name.clone(), asset.source, asset.bytes)));
                    }
                }
            }
            for name in candidates {
                let asset = assets
                    .read(name, MAX_TEXTURE_FILE_BYTES)
                    .map_err(|e| e.to_string())?;
                if let Some(asset) = asset {
                    return Ok(Some((name.clone(), asset.source, asset.bytes)));
                }
            }
            Ok(None)
        })();
        self.idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(assets);
        result
    }
}

enum TexturePreloadOutcome {
    Missing,
    ReadError(String),
    Decoded(Result<(TextureData, f64, f64, u64), String>),
}

struct TexturePreloadResult {
    key: (String, bool, bool),
    read_ms: f64,
    outcome: TexturePreloadOutcome,
}

/// A started `Textures::preload_start` batch awaiting `preload_finish`.
pub struct TexturePreload {
    pending: Vec<crate::map_jobs::JobHandle<TexturePreloadResult>>,
    started: Instant,
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
    /// Earlier decodes of this same map that identical requests may copy.
    seed: Option<TextureSeed>,
    /// Content hash of each image (0 = not a plain decode), aligned with `images`.
    hashes: Vec<u64>,
    /// Hash of the decode that produced the image about to be pushed.
    last_hash: u64,
    /// `videoMap` cinematics. Each owns a placeholder entry in `images` that the
    /// renderer overwrites with decoded frames.
    pub videos: Vec<VideoSource>,
    video_cache: BTreeMap<String, Option<usize>>,
}

/// A looping RoQ cinematic bound to one entry of [`Textures::images`].
#[derive(Debug, Clone)]
pub struct VideoSource {
    pub texture: usize,
    pub name: String,
    pub data: Arc<[u8]>,
}

impl Textures {
    pub fn set_seed(&mut self, seed: TextureSeed) {
        self.seed = Some(seed);
    }

    /// Per-image content hashes for the next preparation's `TextureSeed`.
    pub fn content_hashes(&self) -> Vec<u64> {
        let mut hashes = self.hashes.clone();
        hashes.resize(self.images.len(), 0);
        hashes
    }

    fn push_image(&mut self, image: TextureData) {
        self.hashes.resize(self.images.len(), 0);
        self.hashes.push(std::mem::take(&mut self.last_hash));
        self.images.push(image);
    }

    pub fn new() -> Self {
        Self {
            seed: None,
            hashes: Vec::new(),
            last_hash: 0,
            videos: Vec::new(),
            video_cache: BTreeMap::new(),
            images: Vec::new(),
            cache: BTreeMap::new(),
            source_cache: BTreeMap::new(),
            generated_normal_cache: BTreeMap::new(),
            warnings: Vec::new(),
            load_stats: TextureLoadStats::default(),
        }
    }

    /// Register a `videoMap` cinematic and return its placeholder texture index.
    /// The placeholder is single-mip black at the video's size; the renderer
    /// streams decoded frames into it. Like the engine, a missing `.roq`
    /// extension is defaulted.
    pub fn load_video(&mut self, assets: &mut AssetSearchPath, path: &str) -> Option<usize> {
        let path = path.replace('\\', "/").to_ascii_lowercase();
        if let Some(value) = self.video_cache.get(&path) {
            return *value;
        }
        let name = if path.ends_with(".roq") { path.clone() } else { format!("{path}.roq") };
        let value = match assets.read(&name, MAX_VIDEO_FILE_BYTES) {
            Ok(Some(asset)) => {
                let bytes: Arc<[u8]> = asset.bytes.into();
                match jka_assets::roq::RoqVideo::open(Arc::clone(&bytes)) {
                Ok(video) => {
                    let (width, height) = (video.width(), video.height());
                    let mut rgba = vec![0u8; (width * height * 4) as usize];
                    rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
                    let index = self.images.len();
                    self.push_image(TextureData {
                        label: format!("{name} [video]"),
                        source: Some(asset.source),
                        width,
                        height,
                        rgba,
                        rgba16f: None,
                        mip_level_count: 1,
                        clamp: false,
                        srgb: true,
                    });
                    self.videos.push(VideoSource {
                        texture: index,
                        name: name.clone(),
                        data: bytes,
                    });
                    Some(index)
                }
                Err(error) => {
                    self.warnings.push(format!("{name}: {error}"));
                    None
                }
                }
            }
            Ok(None) => {
                self.warnings.push(format!("{name}: video not found"));
                None
            }
            Err(error) => {
                self.warnings.push(format!("{name}: {error}"));
                None
            }
        };
        self.video_cache.insert(path, value);
        value
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
                self.push_image(image);
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
                self.push_image(image);
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
            self.push_image(image);
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
            self.push_image(image);
            index
        });
        self.cache.insert((path, clamp, true), value);
        value
    }

    /// Read and decode `requests` on the map workers, and wait for them.
    #[allow(dead_code)]
    pub fn preload(
        &mut self,
        assets: &AssetSearchPath,
        requests: &[(String, bool, bool)],
        jobs: &MapJobPool,
    ) -> Result<(), String> {
        let pending = self.preload_start(assets, requests, jobs)?;
        self.preload_finish(pending)
    }

    /// Queue every uncached request on the map workers and return at once. Each
    /// job reads its own file (through a private reader over the shared VFS
    /// index) and decodes it, so the calling thread is free for other work until
    /// `preload_finish`; it must not touch `self` in between.
    pub fn preload_start(
        &mut self,
        assets: &AssetSearchPath,
        requests: &[(String, bool, bool)],
        jobs: &MapJobPool,
    ) -> Result<TexturePreload, String> {
        let started = Instant::now();
        let readers = Arc::new(TextureReaders {
            idle: std::sync::Mutex::new(Vec::new()),
            template: std::sync::Mutex::new(assets.fork()),
            overrides_allowed: assets.asset_overrides_allowed(),
        });
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

            let clamp = *clamp;
            let srgb = *srgb;
            let seed = self.seed.clone();
            let readers = Arc::clone(&readers);
            let handle = jobs.submit(Task::MapTextureDecode, move || {
                let read_started = Instant::now();
                let found = readers.read_first(&candidates);
                let read_ms = read_started.elapsed().as_secs_f64() * 1000.0;
                let outcome = match found {
                    Err(error) => TexturePreloadOutcome::ReadError(error),
                    Ok(None) => TexturePreloadOutcome::Missing,
                    Ok(Some((name, source, bytes))) => TexturePreloadOutcome::Decoded(
                        decode_texture_seeded(seed.as_ref(), &name, &bytes, clamp, srgb).map(
                            |(mut image, decode_ms, mip_ms, hash)| {
                                image.source = Some(source);
                                (image, decode_ms, mip_ms, hash)
                            },
                        ),
                    ),
                };
                TexturePreloadResult { key, read_ms, outcome }
            })?;
            pending.push(handle);
        }
        Ok(TexturePreload { pending, started })
    }

    /// Wait for a `preload_start` batch and install its images.
    pub fn preload_finish(&mut self, preload: TexturePreload) -> Result<(), String> {
        // Resolve in request order so texture indices remain deterministic.
        for handle in preload.pending {
            let TexturePreloadResult { key, read_ms, outcome } = handle.join()?;
            self.load_stats.read_ms += read_ms;
            match outcome {
                // Not found anywhere: leave it uncached so a later load can warn.
                TexturePreloadOutcome::Missing => {}
                TexturePreloadOutcome::ReadError(error) => return Err(error),
                TexturePreloadOutcome::Decoded(Ok((image, decode_ms, mip_ms, hash))) => {
                    let format = match image.label.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
                        "tga" => Some(0),
                        "jpg" | "jpeg" => Some(1),
                        "png" => Some(2),
                        _ => None,
                    };
                    if let Some(format) = format {
                        self.load_stats.format_images[format] += 1;
                        self.load_stats.format_decode_ms[format] += decode_ms;
                    }
                    let index = self.images.len();
                    self.last_hash = hash;
                    self.push_image(image);
                    self.cache.insert(key, Some(index));
                    self.load_stats.decode_ms += decode_ms;
                    self.load_stats.mip_ms += mip_ms;
                    self.load_stats.decoded_images += 1;
                }
                TexturePreloadOutcome::Decoded(Err(error)) => {
                    self.warnings.push(error);
                    self.cache.insert(key, None);
                }
            }
        }
        self.load_stats.preload_wall_ms += preload.started.elapsed().as_secs_f64() * 1000.0;
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
        use rayon::prelude::*;

        let started = Instant::now();
        // Borrow the source rather than cloning it: its RGBA buffer carries the whole
        // mip chain, and this runs once per material base texture.
        let base = self.images.get(base_index)?;
        let width = base.width as usize;
        let height = base.height as usize;
        if width == 0 || height == 0 || base.rgba.len() < width * height * 4 {
            return None;
        }
        let (base_width, base_height) = (base.width, base.height);
        let base_clamp = base.clamp;
        let base_label = base.label.clone();
        let base_source = base.source.clone();

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

        // Run the reference 3x3 Sobel filter over the generated alpha heights. Rows
        // only read the immutable `heights` snapshot, so they filter in parallel with
        // bit-identical output.
        let heights = rgba.clone();
        rgba.par_chunks_mut(width * 4).enumerate().for_each(|(y, row)| {
            for x in 0..width {
                let mut s = [0_f32; 9];
                let mut n = 0;
                for oy in -1_isize..=1 {
                    let sy = coord(y as isize + oy, height, base_clamp);
                    for ox in -1_isize..=1 {
                        let sx = coord(x as isize + ox, width, base_clamp);
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
                let i = x * 4;
                row[i] = encode(normal[0]);
                row[i + 1] = encode(normal[1]);
                row[i + 2] = encode(normal[2]);
                // row[i + 3] intentionally remains the generated height.
            }
        });

        let (rgba, mip_level_count) = build_normal_height_mip_chain(base_width, base_height, rgba);
        let index = self.images.len();
        self.load_stats.generated_normal_ms += started.elapsed().as_secs_f64() * 1000.0;
        self.load_stats.generated_normals += 1;
        self.push_image(TextureData {
            label: format!("{base_label} [Rend2 generated normal]"),
            source: base_source,
            width: base_width,
            height: base_height,
            rgba,
            rgba16f: None,
            mip_level_count,
            clamp: base_clamp,
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
                .read_from_source(name, MAX_TEXTURE_FILE_BYTES, source)
                .map_err(|e| e.to_string())?;
            self.load_stats.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
            let Some(asset) = asset else {
                continue;
            };
            let (mut image, decode_ms, mip_ms, hash) = decode_texture_seeded(
                self.seed.as_ref(), name, &asset.bytes, clamp, srgb,
            )?;
            self.last_hash = hash;
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
                    .read_stock(name, MAX_TEXTURE_FILE_BYTES)
                    .map_err(|e| e.to_string())?;
                self.load_stats.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
                let Some(asset) = asset else {
                    continue;
                };
                let (mut image, decode_ms, mip_ms, hash) = decode_texture_seeded(
                    self.seed.as_ref(), name, &asset.bytes, clamp, srgb,
                )?;
                self.last_hash = hash;
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
                .read(name, MAX_TEXTURE_FILE_BYTES)
                .map_err(|e| e.to_string())?;
            self.load_stats.read_ms += read_started.elapsed().as_secs_f64() * 1000.0;
            let Some(asset) = asset else {
                continue;
            };
            let (mut image, decode_ms, mip_ms, hash) = decode_texture_seeded(
                self.seed.as_ref(), name, &asset.bytes, clamp, srgb,
            )?;
            self.last_hash = hash;
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

/// Decoded images from an earlier preparation of the same map, lent by the
/// app's restart cache. A decode is a pure function of (qpath, file bytes,
/// clamp, sRGB), so an identical request copies the finished image out of the
/// previous map instead of decoding and mipping it again. Nothing is stored
/// beyond the map the app already keeps.
#[derive(Clone)]
pub struct TextureSeed {
    map: std::sync::Arc<crate::scene::PreparedMap>,
    by_hash: std::sync::Arc<std::collections::HashMap<u64, usize>>,
}

impl TextureSeed {
    pub fn new(map: &std::sync::Arc<crate::scene::PreparedMap>) -> Self {
        let by_hash = map
            .texture_hashes
            .iter()
            .enumerate()
            .filter(|&(index, &hash)| hash != 0 && index < map.textures.len())
            .map(|(index, &hash)| (hash, index))
            .collect();
        Self { map: std::sync::Arc::clone(map), by_hash: std::sync::Arc::new(by_hash) }
    }
}

fn texture_content_hash(name: &str, bytes: &[u8], clamp: bool, srgb: bool) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(name.as_bytes());
    hasher.write_u8(u8::from(clamp) | (u8::from(srgb) << 1));
    hasher.write(bytes);
    // 0 means "unknown" in `Textures::hashes`, so never produce it.
    hasher.finish().max(1)
}

/// `decode_texture_data_profiled` with mipmaps, reusing the seed's identical
/// earlier decode when there is one. Also returns the content hash that
/// identifies this decode for the next preparation. A hit reports zero
/// decode/mip time: none was spent.
fn decode_texture_seeded(
    seed: Option<&TextureSeed>,
    name: &str,
    bytes: &[u8],
    clamp: bool,
    srgb: bool,
) -> Result<(TextureData, f64, f64, u64), String> {
    let hash = texture_content_hash(name, bytes, clamp, srgb);
    if let Some(image) = seed
        .and_then(|seed| seed.by_hash.get(&hash).and_then(|&index| seed.map.textures.get(index)))
    {
        return Ok((image.clone(), 0.0, 0.0, hash));
    }
    let (image, decode_ms, mip_ms) = decode_texture_data_profiled(name, bytes, clamp, true, srgb)?;
    Ok((image, decode_ms, mip_ms, hash))
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
        let fill_row = |y: u32, row: &mut [u8]| {
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
                let index = (x * 4) as usize;
                row[index] = encode(normal[0]);
                row[index + 1] = encode(normal[1]);
                row[index + 2] = encode(normal[2]);
                // Rend2's R_MipMapNormalHeight preserves the strongest height
                // sample instead of averaging it away.
                row[index + 3] = max_height;
            }
        };
        let row_bytes = width as usize * 4;
        if u64::from(width) * u64::from(height) >= 16 * 1024 {
            use rayon::prelude::*;
            next.par_chunks_mut(row_bytes)
                .enumerate()
                .for_each(|(y, row)| fill_row(y as u32, row));
        } else {
            for (y, row) in next.chunks_mut(row_bytes).enumerate() {
                fill_row(y as u32, row);
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

/// Box-filtered mip chain (2x2 average, truncating, edge texels repeated for odd
/// sizes), stored base level first. Each level is written straight after the
/// previous one in a single buffer, so nothing is cloned.
fn build_mip_chain(width: u32, height: u32, base: Vec<u8>) -> (Vec<u8>, u32) {
    let mut all = base;
    // A full chain adds a third of the base again.
    all.reserve(all.len() / 3 + 64);
    let mut previous_offset = 0_usize;
    let mut previous_width = width as usize;
    let mut previous_height = height as usize;
    let mut levels = 1_u32;

    while previous_width > 1 || previous_height > 1 {
        let width = (previous_width / 2).max(1);
        let height = (previous_height / 2).max(1);
        let next_offset = all.len();
        all.resize(next_offset + width * height * 4, 0);
        let (done, next) = all.split_at_mut(next_offset);
        let previous = &done[previous_offset..];
        for (y, row) in next.chunks_exact_mut(width * 4).enumerate() {
            let top = &previous[(y * 2).min(previous_height - 1) * previous_width * 4..];
            let bottom = &previous[(y * 2 + 1).min(previous_height - 1) * previous_width * 4..];
            for (x, texel) in row.chunks_exact_mut(4).enumerate() {
                let left = (x * 2).min(previous_width - 1) * 4;
                let right = (x * 2 + 1).min(previous_width - 1) * 4;
                for channel in 0..4 {
                    let sum = u32::from(top[left + channel])
                        + u32::from(top[right + channel])
                        + u32::from(bottom[left + channel])
                        + u32::from(bottom[right + channel]);
                    texel[channel] = (sum / 4) as u8;
                }
            }
        }
        previous_offset = next_offset;
        previous_width = width;
        previous_height = height;
        levels += 1;
    }

    (all, levels)
}

/// The original per-sample implementation, kept to prove the fast one identical.
#[cfg(test)]
fn build_mip_chain_reference(width: u32, height: u32, base: Vec<u8>) -> (Vec<u8>, u32) {
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
        // GLS_ATEST_GE_C0 (OpenJK `GE192`): alpha >= 0xC0 / 255.
        "ge192" => 0.75,
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

/// sRGB byte -> linear value, evaluated with the exact per-pixel formula so a
/// table lookup is bit-identical to computing `powf` for every pixel.
fn srgb_byte_to_linear() -> &'static [f64; 256] {
    static TABLE: std::sync::OnceLock<[f64; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|byte| {
            let srgb = byte as f64 / 255.0;
            if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

/// Nanoseconds spent averaging light images, for the `[MAP MATERIALS]` line.
pub(crate) static LIGHT_IMAGE_AVERAGE_NS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn normalized_texture_average(texture: &TextureData) -> [f32; 3] {
    let started = Instant::now();
    let result = normalized_texture_average_inner(texture);
    LIGHT_IMAGE_AVERAGE_NS.fetch_add(
        started.elapsed().as_nanos() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
    result
}

fn normalized_texture_average_inner(texture: &TextureData) -> [f32; 3] {
    let linear = srgb_byte_to_linear();
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
            sum[channel] += linear[usize::from(pixel[channel])];
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

    let linear = srgb_byte_to_linear();
    let mut weighted_rgb = [0.0_f64; 3];
    let mut luminance_sum = 0.0_f64;
    let mut active = 0_u64;
    for pixel in texture.rgba[..base_bytes].chunks_exact(4) {
        let rgb = [pixel[0], pixel[1], pixel[2]].map(|channel| linear[usize::from(channel)]);
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
    omit_environment_stages: bool,
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
                    if omit_environment_stages && matches!(stage.tc_gen, TcGen::Environment) {
                        continue;
                    }
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

/// The stages of a sky shader that authored a `skyParms` cloud height. The
/// engine draws every one of them over the outer box as a cloud layer, so they
/// keep their own blend, rgbGen/alphaGen and tcMods. They carry no PBR companion
/// maps, lightmap or surface-sprite behaviour: none of that applies to a sky.
fn sky_cloud_stages(
    name: &str,
    definition: &Shader,
    assets: &mut AssetSearchPath,
    textures: &mut Textures,
    preferred_source: Option<&std::path::Path>,
) -> Vec<MaterialStage> {
    let mut stages = Vec::new();
    for source in &definition.stages {
        if source.image.is_empty()
            || source.image == "$lightmap"
            || source.surface_sprite.is_some()
        {
            continue;
        }
        let texture = match source.image.as_str() {
            "$whiteimage" | "*white" => StageTexture::White,
            value if value.starts_with('$') => {
                textures.warnings.push(format!(
                    "{name}: unsupported generated image {value}; using white"
                ));
                StageTexture::White
            }
            value => preferred_source
                .and_then(|provider| textures.load_from_source(assets, value, source.clamp, provider))
                .or_else(|| textures.load(assets, value, source.clamp))
                .map(StageTexture::Image)
                .unwrap_or(StageTexture::White),
        };
        let blend = stage_blend(source);
        if !source.blend.is_empty() && blend.is_none() {
            textures.warnings.push(format!(
                "{name}: unsupported blendFunc {}; rendering stage opaque",
                source.blend
            ));
        }
        stages.push(MaterialStage {
            texture,
            enhancements: StageEnhancements::default(),
            blend,
            alpha_cutoff: alpha_cutoff(source),
            opacity: source.alpha.unwrap_or(1.0),
            color: source.color.unwrap_or([1.0; 3]),
            rgb_gen: source.rgb_gen,
            alpha_gen: source.alpha_gen,
            tc_gen: source.tc_gen,
            tc_mods: source.tc_mods.clone(),
            depth_write: source.depth_write,
            depth_equal: source.depth_equal,
        });
    }
    stages
}

pub fn describe(
    name: &str,
    flags: u32,
    definition: Option<&Shader>,
    origin: Option<&ShaderDefinitionOrigin>,
    assets: &mut AssetSearchPath,
    textures: &mut Textures,
    gen_normal_maps: bool,
    omit_environment_stages: bool,
) -> SurfaceMaterial {
    let empty = Shader::default();
    let definition = definition.unwrap_or(&empty);
    let explicit = !definition.stages.is_empty();
    let stage_is_omitted = |stage: &shader::Stage| {
        omit_environment_stages && matches!(stage.tc_gen, TcGen::Environment)
    };
    let preferred_source = origin
        .filter(|origin| origin.mtr_override)
        .map(|origin| origin.source.as_path());
    let sky = flags & SURF_SKY != 0 || definition.sky;
    // A `surfaceparm fog` shader without stages only defines a fog volume; the
    // engine never draws its brush faces or q3map2's fog hull. Without this the
    // stageless-shader fallback below turns the hull into an opaque white box
    // that encloses the viewer and hides the sky.
    let stageless_fog = definition.fog && definition.stages.is_empty();
    let base_hidden = definition.nodraw || flags & SURF_NODRAW != 0 || stageless_fog;
    let surface_sprite_cull_quirk = definition
        .stages
        .iter()
        .filter(|stage| !stage_is_omitted(stage))
        .any(|stage| stage.surface_sprite.is_some());
    let mut surface_light = (definition.surface_light > 0.0).then(|| {
        // q3map uses q3map_lightimage for emitter color when present; without
        // one, its shader info falls back to the surface image's average color.
        let color_image = definition.light_image.as_deref().or_else(|| {
            definition
                .stages
                .iter()
                .filter(|stage| !stage_is_omitted(stage))
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
    // Removing a stage must not accidentally remove the opaque/depth seed for
    // the whole material. Scene preparation intentionally gives only the first
    // ordinary unblended stage implicit depthWrite, so remember whether the
    // authored first ordinary stage was an environment pass that supplied that
    // coverage. The first surviving ordinary stage can then inherit the base
    // role on the cold material-specialization path.
    let mut saw_authored_ordinary_stage = false;
    let mut omitted_front_depth_base = false;

    if !sky && !base_hidden {
        if explicit {
            for source in &definition.stages {
                let procedural_sprite_only = source.surface_sprite.is_some_and(|sprite| {
                    matches!(
                        sprite.kind,
                        shader::SurfaceSpriteType::Vertical | shader::SurfaceSpriteType::Effect
                    )
                });
                let authored_ordinary_stage = !source.image.is_empty() && !procedural_sprite_only;
                if stage_is_omitted(source) {
                    if authored_ordinary_stage && !saw_authored_ordinary_stage {
                        omitted_front_depth_base =
                            source.depth_write || stage_blend(source).is_none();
                    }
                    saw_authored_ordinary_stage |= authored_ordinary_stage;
                    continue;
                }
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
                    value if source.video => textures
                        .load_video(assets, value)
                        .map(StageTexture::Image)
                        .unwrap_or(StageTexture::White),
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
                let promote_to_base = omitted_front_depth_base && stages.is_empty();
                let mut blend = stage_blend(source);
                if !source.blend.is_empty() && blend.is_none() {
                    textures.warnings.push(format!(
                        "{name}: unsupported blendFunc {}; rendering stage opaque",
                        source.blend
                    ));
                }
                if promote_to_base {
                    // This stage was authored to composite over the removed
                    // environment base. Make it the new opaque base instead of
                    // blending it against whatever happened to be rendered
                    // behind the surface. This restores both color opacity and
                    // main-scene depth occlusion without a runtime shader branch.
                    blend = None;
                }
                let base_index = match texture {
                    // A video frame has no companion maps to look up or derive.
                    StageTexture::Image(_) if source.video => None,
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
                    depth_write: source.depth_write || promote_to_base,
                    // A later pass commonly authors depthFunc equal because the
                    // removed base already populated depth. Once promoted to the
                    // base pass there is no earlier depth value to compare with.
                    depth_equal: source.depth_equal && !promote_to_base,
                });
                saw_authored_ordinary_stage |= authored_ordinary_stage;
            }

            // A non-translucent shader made entirely from tcGen environment
            // stages must still be solid when reflections are strictly Off.
            // Prefer its same-named diffuse image as a non-reflective fallback;
            // if the asset does not exist, white still preserves depth/occlusion
            // instead of turning a wall into a hole. This exists only in the
            // specialized Off material set.
            if omit_environment_stages
                && omitted_front_depth_base
                && stages.is_empty()
                && !definition.translucent
                && !definition.water
            {
                let texture = textures
                    .load(assets, name, false)
                    .map(StageTexture::Image)
                    .unwrap_or(StageTexture::White);
                stages.push(MaterialStage {
                    texture,
                    enhancements: StageEnhancements::default(),
                    blend: None,
                    alpha_cutoff: 0.0,
                    opacity: 1.0,
                    color: [1.0; 3],
                    rgb_gen: RgbGen::Identity,
                    alpha_gen: AlphaGen::Identity,
                    tc_gen: TcGen::Base,
                    tc_mods: Vec::new(),
                    depth_write: true,
                    depth_equal: false,
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

    if sky && definition.sky_cloud_height > 0.0 && explicit {
        stages = sky_cloud_stages(name, definition, assets, textures, preferred_source);
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
        sky_cloud_height: if sky { definition.sky_cloud_height } else { 0.0 },
        explicit,
        // JKA's `detail` keyword is only a stage-enable marker. For the
        // fallback AUTO detail-texture path, only that authored marker counts
        // as an existing detail stage. Do not infer authored detail from shader
        // or texture filenames; if a map omitted the marker, we still treat the
        // material as lacking authored detail and fall back to MATERIAL_* or
        // the generic AUTO texture choice.
        authored_detail: definition.stages.iter().any(|stage| stage.detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_matches_the_reference_for_odd_and_thin_sizes() {
        let mut state = 0x2545_f491_u32;
        for (width, height) in [(1, 1), (2, 2), (4, 1), (1, 4), (3, 5), (7, 2), (64, 64), (33, 17), (256, 128), (5, 1)] {
            let base: Vec<u8> = (0..width * height * 4)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    (state >> 8) as u8
                })
                .collect();
            assert_eq!(
                build_mip_chain(width, height, base.clone()),
                build_mip_chain_reference(width, height, base),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn alpha_func_cutoffs_match_the_stock_tests() {
        let cutoff = |func: &str| {
            alpha_cutoff(&shader::Stage { alpha_test: func.to_owned(), ..Default::default() })
        };
        assert_eq!(cutoff("ge128"), 0.5);
        assert_eq!(cutoff("ge192"), 0.75);
        assert_eq!(cutoff("gt0"), 1.0 / 255.0);
        assert_eq!(cutoff(""), 0.0);
    }

    fn material_with_environment_policy(
        text: &str,
        omit_environment_stages: bool,
    ) -> SurfaceMaterial {
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
            omit_environment_stages,
        )
    }

    fn material(text: &str) -> SurfaceMaterial {
        material_with_environment_policy(text, false)
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
    fn reflection_off_omits_environment_stage_without_touching_siblings() {
        let material = material_with_environment_policy(
            "test {\n{\nmap textures/factory/enviro\ntcGen environment\n}\n{\nmap textures/imperial/square\nblendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA\n}\n{\nmap $lightmap\nblendFunc GL_DST_COLOR GL_ZERO\n}\n}",
            true,
        );
        assert!(material.explicit);
        assert_eq!(material.stages.len(), 2);
        assert!(material
            .stages
            .iter()
            .all(|stage| !matches!(stage.tc_gen, TcGen::Environment)));
        assert!(matches!(material.stages[1].tc_gen, TcGen::Lightmap));
    }

    #[test]
    fn reflection_off_promotes_survivor_after_opaque_environment_base() {
        let material = material_with_environment_policy(
            "test {\n{\nmap textures/factory/enviro\ntcGen environment\n}\n{\nmap textures/imperial/square\nblendFunc blend\ndepthFunc equal\n}\n{\nmap $lightmap\nblendFunc filter\n}\n}",
            true,
        );
        assert_eq!(material.stages.len(), 2);
        assert!(material.stages[0].blend.is_none());
        assert!(material.stages[0].depth_write);
        assert!(!material.stages[0].depth_equal);
        assert!(matches!(material.stages[0].tc_gen, TcGen::Base));
        assert!(material.stages[1].blend.is_some());
        assert!(matches!(material.stages[1].tc_gen, TcGen::Lightmap));
    }

    #[test]
    fn reflection_off_keeps_environment_only_opaque_surface_as_occluder() {
        let material = material_with_environment_policy(
            "test {\n{\nmap textures/factory/enviro\ntcGen environment\n}\n}",
            true,
        );
        assert_eq!(material.stages.len(), 1);
        assert!(material.stages[0].blend.is_none());
        assert!(material.stages[0].depth_write);
        assert!(!material.stages[0].depth_equal);
        assert!(matches!(material.stages[0].tc_gen, TcGen::Base));
        assert!(!material.hidden);
    }

    #[test]
    fn detail_named_stage_blocks_fallback_without_detail_keyword() {
        let material = material(
            "test {\n{\ndepthFunc equal\nmap textures/theisland4097/detail_l0.jpg\nrgbGen identity\ntcMod scale 8 8\n}\n}",
        );
        assert!(material.authored_detail);
        assert_eq!(material.stages.len(), 1);
        assert!(material.stages[0].depth_equal);
        assert!(matches!(
            material.stages[0].tc_mods.as_slice(),
            [TcMod::Scale(8.0, 8.0)]
        ));
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
