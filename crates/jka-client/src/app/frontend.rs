use jka_assets::pk3::AssetSearchPath;
use jka_protocol::{
    commands::{atoi, info_value, tokenize, BigConfigOutcome, BigConfigString},
    demo::DemoReader,
    server::{Decoder as ServerMessageDecoder, Event as ServerMessageEvent},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    path::Path,
};

use crate::cgame::{CS_PLAYERS, ET_EVENTS};

const LEVELSHOT_LIMIT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrontendPage {
    Main,
    Play,
    ServerBrowser,
    SoloGame,
    PlayDemo,
    Controls,
    DeveloperTools,
    AssetViewer,
    MapViewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AssetKind {
    Md3,
    Glm,
    Efx,
    Shader,
}

impl AssetKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Md3 => "MD3",
            Self::Glm => "GLM",
            Self::Efx => "EFX",
            Self::Shader => "SHADER",
        }
    }

    pub fn is_model(self) -> bool {
        matches!(self, Self::Md3 | Self::Glm)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AssetFilter {
    All,
    Models,
    Md3,
    Glm,
    Efx,
    Shader,
}

impl Default for AssetFilter {
    fn default() -> Self {
        Self::All
    }
}

impl AssetFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All assets",
            Self::Models => "Models",
            Self::Md3 => "MD3",
            Self::Glm => "GLM",
            Self::Efx => "EFX",
            Self::Shader => "Shaders",
        }
    }

    pub fn matches(self, kind: AssetKind) -> bool {
        match self {
            Self::All => true,
            Self::Models => kind.is_model(),
            Self::Md3 => kind == AssetKind::Md3,
            Self::Glm => kind == AssetKind::Glm,
            Self::Efx => kind == AssetKind::Efx,
            Self::Shader => kind == AssetKind::Shader,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct AssetEntry {
    /// Stable browser identity. For shader definitions this includes both the
    /// physical .shader qpath and the definition name; for other assets it is
    /// simply the qpath.
    pub id: String,
    /// Physical VFS asset that owns this entry.
    pub qpath: String,
    pub kind: AssetKind,
    pub folder: String,
    pub leaf: String,
    pub display_name: String,
    /// Individual material name when this row represents one definition
    /// inside a .shader file.
    pub shader_name: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct AssetDetail {
    pub qpath: String,
    pub kind: AssetKind,
    pub source: String,
    pub size_bytes: usize,
    pub stats: Vec<(String, String)>,
    pub items: Vec<String>,
    pub raw_text: Option<String>,
    /// Shader definitions contained by a selected .shader file. The viewer
    /// uses these as real material preview targets instead of showing only
    /// source text.
    pub shader_names: Vec<String>,
    /// Suggested repeat cycle for an EFX preview. This is derived from the
    /// longest authored spawn/lifetime and the file's repeatDelay.
    pub preview_cycle_ms: Option<i32>,
    /// Model-space center used by the live preview to frame arbitrary assets.
    pub model_center: Option<[f32; 3]>,
    /// Bounding radius around `model_center` for automatic preview framing.
    pub model_radius: Option<f32>,
}

#[derive(Debug, Clone)]
pub(super) struct SoloMapEntry {
    /// Package-relative map name without `maps/` or `.bsp`, e.g. `mp/ffa3`.
    pub map_name: String,
    /// Compressed levelshot asset. Decoded lazily only for the selected map.
    pub levelshot: Option<LevelshotAsset>,
}

#[derive(Debug, Clone)]
pub(super) struct DemoEntry {
    /// Package-relative demo name without `demos/` or `.dm_26`.
    pub demo_name: String,
}

#[derive(Debug, Clone)]
pub(super) struct LevelshotAsset {
    pub bytes: Vec<u8>,
    pub format: image::ImageFormat,
}

pub(super) fn scan_solo_maps(
    base: &Path,
    game: Option<&Path>,
) -> Result<Vec<SoloMapEntry>, String> {
    let mut assets = AssetSearchPath::open_game(base, game).map_err(|error| error.to_string())?;
    let map_names: Vec<String> = assets
        .names()
        .filter_map(|name| {
            let lower = name.to_ascii_lowercase();
            lower
                .strip_prefix("maps/")
                .and_then(|name| name.strip_suffix(".bsp"))
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .collect();

    let mut maps = Vec::with_capacity(map_names.len());
    for map_name in map_names {
        let leaf = map_name.rsplit('/').next().unwrap_or(&map_name);
        let mut levelshot = None;

        // JKA levelshots usually mirror the map qpath after maps/, e.g.
        // maps/mp/ffa3.bsp -> levelshots/mp/ffa3.jpg. Keep a leaf-name
        // fallback for custom packs that flatten their levelshots directory.
        'search: for stem in [map_name.as_str(), leaf] {
            for (extension, format) in [
                ("jpg", image::ImageFormat::Jpeg),
                ("jpeg", image::ImageFormat::Jpeg),
                ("png", image::ImageFormat::Png),
                ("tga", image::ImageFormat::Tga),
            ] {
                let candidate = format!("levelshots/{stem}.{extension}");
                match assets.read(&candidate, LEVELSHOT_LIMIT_BYTES) {
                    Ok(Some(asset)) => {
                        levelshot = Some(LevelshotAsset {
                            bytes: asset.bytes,
                            format,
                        });
                        break 'search;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!("Levelshot {candidate}: {error}");
                    }
                }
            }
        }

        maps.push(SoloMapEntry {
            map_name,
            levelshot,
        });
    }

    maps.sort_by(|a, b| a.map_name.cmp(&b.map_name));
    Ok(maps)
}

pub(super) fn scan_source_maps(
    base: &Path,
    game: Option<&Path>,
) -> Result<Vec<SoloMapEntry>, String> {
    let mut assets = AssetSearchPath::open_game(base, game).map_err(|error| error.to_string())?;
    let map_names: Vec<String> = assets
        .names()
        .filter_map(|name| {
            let lower = name.to_ascii_lowercase();
            lower
                .strip_prefix("maps/")
                .and_then(|name| name.strip_suffix(".map"))
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .collect();

    let mut maps = Vec::with_capacity(map_names.len());
    for map_name in map_names {
        let leaf = map_name.rsplit('/').next().unwrap_or(&map_name);
        let mut levelshot = None;
        'search: for stem in [map_name.as_str(), leaf] {
            for (extension, format) in [
                ("jpg", image::ImageFormat::Jpeg),
                ("jpeg", image::ImageFormat::Jpeg),
                ("png", image::ImageFormat::Png),
                ("tga", image::ImageFormat::Tga),
            ] {
                let candidate = format!("levelshots/{stem}.{extension}");
                match assets.read(&candidate, LEVELSHOT_LIMIT_BYTES) {
                    Ok(Some(asset)) => {
                        levelshot = Some(LevelshotAsset { bytes: asset.bytes, format });
                        break 'search;
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("Levelshot {candidate}: {error}"),
                }
            }
        }
        maps.push(SoloMapEntry { map_name, levelshot });
    }
    maps.sort_by(|a, b| a.map_name.cmp(&b.map_name));
    Ok(maps)
}

pub(super) fn scan_asset_viewer_assets(
    base: &Path,
    game: Option<&Path>,
) -> Result<Vec<AssetEntry>, String> {
    const SHADER_CATALOG_READ_LIMIT_BYTES: usize = 16 * 1024 * 1024;

    let mut assets = AssetSearchPath::open_game(base, game).map_err(|error| error.to_string())?;
    // Snapshot names before reading shader files because reads require mutable
    // VFS access. Shader files expand into one browser entry per definition;
    // the .shader container itself is only a filter/source label.
    let names = assets.names().map(str::to_owned).collect::<Vec<_>>();
    let mut entries = Vec::new();

    for name in names {
        let lower = name.to_ascii_lowercase();
        let (folder, leaf) = name
            .rsplit_once('/')
            .map_or(("", name.as_str()), |(folder, leaf)| (folder, leaf));

        if lower.ends_with(".shader") {
            let asset = match assets.read(&name, SHADER_CATALOG_READ_LIMIT_BYTES) {
                Ok(Some(asset)) => asset,
                Ok(None) => continue,
                Err(error) => {
                    // One malformed/unreadable package entry should not make the
                    // entire developer browser unusable. Keep the rest of the
                    // shader catalog and report this source in the console.
                    eprintln!("ASSET VIEWER shader catalog read {name}: {error}");
                    continue;
                }
            };
            let text = String::from_utf8_lossy(&asset.bytes);
            match jka_assets::shader::parse(&text) {
                Ok(parsed) => {
                    for shader_name in parsed.keys() {
                        let display_name = shader_name
                            .rsplit('/')
                            .next()
                            .filter(|part| !part.is_empty())
                            .unwrap_or(shader_name)
                            .to_owned();
                        entries.push(AssetEntry {
                            id: format!("{name}::{shader_name}"),
                            qpath: name.clone(),
                            kind: AssetKind::Shader,
                            folder: folder.to_owned(),
                            leaf: leaf.to_owned(),
                            display_name,
                            shader_name: Some(shader_name.clone()),
                        });
                    }
                }
                Err(error) => {
                    eprintln!("ASSET VIEWER shader catalog parse {name}: {error}");
                }
            }
            continue;
        }

        let kind = if lower.ends_with(".md3") {
            AssetKind::Md3
        } else if lower.ends_with(".glm") {
            AssetKind::Glm
        } else if lower.ends_with(".efx") {
            AssetKind::Efx
        } else {
            continue;
        };
        let stem = leaf.rsplit_once('.').map_or(leaf, |(stem, _)| stem);
        let display_name = if kind.is_model() && stem.eq_ignore_ascii_case("model") {
            folder
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(stem)
                .to_owned()
        } else {
            stem.to_owned()
        };
        let folder = folder.to_owned();
        let leaf = leaf.to_owned();
        entries.push(AssetEntry {
            id: name.clone(),
            qpath: name,
            kind,
            folder,
            leaf,
            display_name,
            shader_name: None,
        });
    }

    // Asset Viewer is an A-Z browser. Sort by the name actually shown in the
    // grid so the alphabet rail and visual order always agree. Generic JKA
    // model.glm/model.md3 files use their parent folder as the model name.
    entries.sort_by(|a, b| {
        a.display_name
            .to_ascii_lowercase()
            .cmp(&b.display_name.to_ascii_lowercase())
            .then_with(|| {
                a.shader_name
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.shader_name.as_deref().unwrap_or(""))
            })
            .then_with(|| a.qpath.cmp(&b.qpath))
    });
    Ok(entries)
}

const ASSET_VIEWER_READ_LIMIT_BYTES: usize = 128 * 1024 * 1024;

pub(super) fn inspect_asset(
    base: &Path,
    game: Option<&Path>,
    entry: &AssetEntry,
) -> Result<AssetDetail, String> {
    let mut assets = AssetSearchPath::open_game(base, game).map_err(|error| error.to_string())?;
    let asset = assets
        .read(&entry.qpath, ASSET_VIEWER_READ_LIMIT_BYTES)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{} disappeared from the active asset path", entry.qpath))?;
    let source = asset.source.display().to_string();
    let size_bytes = asset.bytes.len();

    match entry.kind {
        AssetKind::Md3 => {
            let model = jka_assets::md3::parse(&asset.bytes)?;
            let frame = model.frames.first();
            let (center, radius) = frame.map_or(([0.0; 3], 1.0), |frame| {
                let center = std::array::from_fn(|i| (frame.mins[i] + frame.maxs[i]) * 0.5);
                let radius = frame.radius.max(1.0);
                (center, radius)
            });
            let triangles = model.surfaces.iter().map(|surface| surface.indices.len() / 3).sum::<usize>();
            let vertices_per_frame = model
                .surfaces
                .iter()
                .map(|surface| surface.frames.first().map_or(surface.vertices.len(), Vec::len))
                .sum::<usize>();
            let shader_refs = model
                .surfaces
                .iter()
                .map(|surface| surface.shader.clone())
                .filter(|shader| !shader.is_empty())
                .collect::<BTreeSet<_>>();
            let mut items = model
                .surfaces
                .iter()
                .map(|surface| format!("{} — {} tris, {} verts — {}", surface.name, surface.indices.len() / 3, surface.vertices.len(), if surface.shader.is_empty() { "<no shader>" } else { &surface.shader }))
                .collect::<Vec<_>>();
            if items.is_empty() {
                items.push("No renderable surfaces".to_owned());
            }
            Ok(AssetDetail {
                qpath: entry.qpath.clone(),
                kind: entry.kind,
                source,
                size_bytes,
                stats: vec![
                    ("Frames".into(), model.frames.len().to_string()),
                    ("Tags / frame".into(), model.tags.first().map_or(0, Vec::len).to_string()),
                    ("Surfaces".into(), model.surfaces.len().to_string()),
                    ("Triangles".into(), triangles.to_string()),
                    ("Vertices / frame".into(), vertices_per_frame.to_string()),
                    ("Shader refs".into(), shader_refs.len().to_string()),
                    ("Radius".into(), format!("{radius:.2}")),
                ],
                items,
                raw_text: None,
                shader_names: Vec::new(),
                preview_cycle_ms: None,
                model_center: Some(center),
                model_radius: Some(radius),
            })
        }
        AssetKind::Glm => {
            let model = jka_assets::ghoul2::parse_glm(&asset.bytes)?;
            let first_lod = model.lods.first();
            let triangles = first_lod
                .map(|lod| lod.surfaces.iter().map(|surface| surface.triangles.len()).sum::<usize>())
                .unwrap_or(0);
            let vertices = first_lod
                .map(|lod| lod.surfaces.iter().map(|surface| surface.vertices.len()).sum::<usize>())
                .unwrap_or(0);
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            if let Some(lod) = first_lod {
                for surface in &lod.surfaces {
                    for vertex in &surface.vertices {
                        for axis in 0..3 {
                            min[axis] = min[axis].min(vertex.position[axis]);
                            max[axis] = max[axis].max(vertex.position[axis]);
                        }
                    }
                }
            }
            let valid_bounds = min.iter().all(|v| v.is_finite()) && max.iter().all(|v| v.is_finite());
            let center = if valid_bounds {
                std::array::from_fn(|i| (min[i] + max[i]) * 0.5)
            } else {
                [0.0; 3]
            };
            let radius = if valid_bounds {
                first_lod
                    .into_iter()
                    .flat_map(|lod| lod.surfaces.iter())
                    .flat_map(|surface| surface.vertices.iter())
                    .map(|vertex| {
                        let d = std::array::from_fn::<_, 3, _>(|i| vertex.position[i] - center[i]);
                        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
                    })
                    .fold(1.0_f32, f32::max)
            } else {
                1.0
            };
            let shader_refs = model
                .hierarchy
                .iter()
                .map(|surface| surface.shader.clone())
                .filter(|shader| !shader.is_empty())
                .collect::<BTreeSet<_>>();
            // Player/NPC model.glm files commonly carry no embedded shader
            // names at all. OpenJK resolves those surfaces through the sibling
            // model_default.skin instead, so expose that separately rather than
            // making "Shader refs 0" look like a broken model.
            let default_skin_qpath = entry
                .leaf
                .eq_ignore_ascii_case("model.glm")
                .then(|| format!("{}/model_default.skin", entry.folder.trim_end_matches('/')))
                .filter(|qpath| !qpath.starts_with('/'));
            let (default_skin_status, skin_shader_refs) = if let Some(skin_qpath) = default_skin_qpath.as_deref() {
                match assets.read(skin_qpath, 4 * 1024 * 1024) {
                    Ok(Some(skin_asset)) => match jka_assets::skin::parse_skin(&skin_asset.bytes) {
                        Ok(skin) => {
                            let refs = skin
                                .iter()
                                .map(|surface| surface.shader.as_str())
                                .filter(|shader| !shader.is_empty() && !shader.eq_ignore_ascii_case("*off"))
                                .collect::<BTreeSet<_>>()
                                .len();
                            (skin_qpath.to_owned(), refs)
                        }
                        Err(error) => (format!("{skin_qpath} (parse error: {error})"), 0),
                    },
                    Ok(None) => ("not found".to_owned(), 0),
                    Err(error) => (format!("{skin_qpath} (read error: {error})"), 0),
                }
            } else {
                ("n/a".to_owned(), 0)
            };
            let items = model
                .hierarchy
                .iter()
                .enumerate()
                .map(|(index, surface)| {
                    let geometry = first_lod
                        .and_then(|lod| lod.surfaces.iter().find(|candidate| candidate.surface_index == index));
                    format!(
                        "{} — {} tris, {} verts — {}",
                        surface.name,
                        geometry.map_or(0, |surface| surface.triangles.len()),
                        geometry.map_or(0, |surface| surface.vertices.len()),
                        if surface.shader.is_empty() { "<no shader>" } else { &surface.shader },
                    )
                })
                .collect();
            Ok(AssetDetail {
                qpath: entry.qpath.clone(),
                kind: entry.kind,
                source,
                size_bytes,
                stats: vec![
                    ("LODs".into(), model.lods.len().to_string()),
                    ("Surfaces".into(), model.hierarchy.len().to_string()),
                    ("Bones".into(), model.num_bones.to_string()),
                    ("LOD0 triangles".into(), triangles.to_string()),
                    ("LOD0 vertices".into(), vertices.to_string()),
                    ("Embedded shader refs".into(), shader_refs.len().to_string()),
                    ("Default skin".into(), default_skin_status),
                    ("Skin shader refs".into(), skin_shader_refs.to_string()),
                    ("Animation".into(), model.anim_name.clone()),
                    ("Radius".into(), format!("{radius:.2}")),
                ],
                items,
                raw_text: None,
                shader_names: Vec::new(),
                preview_cycle_ms: None,
                model_center: Some(center),
                model_radius: Some(radius),
            })
        }
        AssetKind::Efx => {
            let root = crate::fx::gp2::parse(&asset.bytes);
            let effect = crate::fx::template::parse_effect(&entry.qpath, &root);
            let mut counts = BTreeMap::<String, usize>::new();
            for primitive in &effect.primitives {
                *counts.entry(format!("{:?}", primitive.kind)).or_default() += 1;
            }
            let mut stats = vec![
                ("Primitives".into(), effect.primitives.len().to_string()),
                ("Repeat delay".into(), format!("{} ms", effect.repeat_delay)),
                ("Root groups".into(), root.groups.len().to_string()),
            ];
            stats.extend(counts.into_iter().map(|(kind, count)| (kind, count.to_string())));
            let items = effect
                .primitives
                .iter()
                .enumerate()
                .map(|(index, primitive)| {
                    format!(
                        "#{:02} {:?} — media {} — life {:.0}..{:.0} ms — count {:.1}..{:.1}",
                        index + 1,
                        primitive.kind,
                        primitive.media.len(),
                        primitive.life.min,
                        primitive.life.max,
                        primitive.spawn_count.min,
                        primitive.spawn_count.max,
                    )
                })
                .collect();
            let longest_primitive_ms = effect
                .primitives
                .iter()
                .map(|primitive| (primitive.spawn_delay.max + primitive.life.max).ceil() as i32)
                .max()
                .unwrap_or(500);
            let preview_cycle_ms = effect
                .repeat_delay
                .max(longest_primitive_ms.saturating_add(250))
                .clamp(250, 15_000);
            Ok(AssetDetail {
                qpath: entry.qpath.clone(),
                kind: entry.kind,
                source,
                size_bytes,
                stats,
                items,
                raw_text: Some(String::from_utf8_lossy(&asset.bytes).into_owned()),
                shader_names: Vec::new(),
                preview_cycle_ms: Some(preview_cycle_ms),
                model_center: None,
                model_radius: None,
            })
        }
        AssetKind::Shader => {
            let text = String::from_utf8_lossy(&asset.bytes).into_owned();
            let parsed = jka_assets::shader::parse(&text)?;
            let shader_name = entry
                .shader_name
                .as_deref()
                .ok_or_else(|| format!("{} has no selected shader definition", entry.qpath))?;
            let shader = parsed
                .get(shader_name)
                .ok_or_else(|| format!("{shader_name} disappeared from {}", entry.qpath))?;
            let mut flags = Vec::new();
            if shader.sky { flags.push("sky"); }
            if shader.portal { flags.push("portal"); }
            if shader.water { flags.push("water"); }
            if shader.translucent { flags.push("translucent"); }
            if shader.nodraw { flags.push("nodraw"); }
            let mut items = shader
                .stages
                .iter()
                .enumerate()
                .map(|(index, stage)| {
                    format!(
                        "Stage {} — image {} — blend {}{}",
                        index + 1,
                        if stage.image.is_empty() { "<none>" } else { stage.image.as_str() },
                        if stage.blend.is_empty() { "opaque" } else { stage.blend.as_str() },
                        if stage.alpha_test.trim().is_empty() {
                            String::new()
                        } else {
                            format!(" — alpha {}", stage.alpha_test)
                        },
                    )
                })
                .collect::<Vec<_>>();
            if items.is_empty() {
                items.push("No render stages".to_owned());
            }
            Ok(AssetDetail {
                qpath: entry.qpath.clone(),
                kind: entry.kind,
                source,
                size_bytes,
                stats: vec![
                    ("Shader".into(), shader_name.to_owned()),
                    ("Shader file".into(), entry.qpath.clone()),
                    ("Definitions in file".into(), parsed.len().to_string()),
                    ("Stages".into(), shader.stages.len().to_string()),
                    ("Flags".into(), if flags.is_empty() { "none".into() } else { flags.join(", ") }),
                    ("Unsupported directives".into(), shader.unsupported.len().to_string()),
                ],
                items,
                raw_text: Some(text),
                shader_names: vec![shader_name.to_owned()],
                preview_cycle_ms: None,
                model_center: None,
                model_radius: None,
            })
        }
    }
}

pub(super) fn scan_demos(
    base: &Path,
    game: Option<&Path>,
) -> Result<Vec<DemoEntry>, String> {
    let assets = AssetSearchPath::open_game(base, game).map_err(|error| error.to_string())?;
    let mut demos: Vec<DemoEntry> = assets
        .names()
        .filter_map(|name| {
            let lower = name.to_ascii_lowercase();
            lower
                .strip_prefix("demos/")
                .and_then(|name| name.strip_suffix(".dm_26"))
                .filter(|name| !name.is_empty())
                .map(|demo_name| DemoEntry {
                    demo_name: demo_name.to_owned(),
                })
        })
        .collect();

    demos.sort_by(|a, b| a.demo_name.cmp(&b.demo_name));
    Ok(demos)
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DemoConsoleKind {
    Print,
    Chat,
    CenterPrint,
    Event,
}

impl DemoConsoleKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Print => "PRINT",
            Self::Chat => "CHAT",
            Self::CenterPrint => "CENTER",
            Self::Event => "EVENT",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct DemoConsoleEntry {
    pub elapsed_ms: i32,
    pub kind: DemoConsoleKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub(super) struct DemoMapUsage {
    pub map_name: String,
    pub duration_ms: i32,
}

#[derive(Debug, Clone)]
pub(super) struct DemoPlayerUsage {
    pub name: String,
    pub model: String,
    pub duration_ms: i32,
    pub first_seen_ms: i32,
    pub last_seen_ms: i32,
    pub teams: Vec<i32>,
}

#[derive(Debug, Clone)]
pub(super) struct DemoRoundStats {
    pub index: usize,
    pub map_name: String,
    pub start_ms: i32,
    pub duration_ms: i32,
    pub kills: usize,
    pub players: usize,
    pub player_names: Vec<String>,
    pub team_scores: Option<[i32; 2]>,
    pub scores: Vec<(String, i32)>,
    pub leader: Option<(String, i32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DemoDuelKind {
    GameRound,
    PrivatePov,
}

impl DemoDuelKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::GameRound => "DUEL ROUND",
            Self::PrivatePov => "PRIVATE (POV)",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct DemoDuelStats {
    pub index: usize,
    pub kind: DemoDuelKind,
    pub start_ms: i32,
    pub duration_ms: i32,
    pub players: Vec<String>,
    pub winner: Option<String>,
    pub scores: Vec<(String, i32)>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DemoKillMarker {
    pub server_time: i32,
    pub target: i32,
    pub attacker: i32,
    pub means_of_death: i32,
    pub attacker_team: i32,
}

#[derive(Debug, Clone, Default)]
pub(super) struct DemoIndex {
    pub first_active_server_time: Option<i32>,
    pub last_server_time: Option<i32>,
    pub kill_markers: Vec<DemoKillMarker>,
    /// Timeline-addressable transient notices used to rebuild HUD state after seeks.
    pub transient_notices: Vec<DemoConsoleEntry>,
}

impl DemoIndex {
    pub fn duration_ms(&self) -> i32 {
        match (self.first_active_server_time, self.last_server_time) {
            (Some(first), Some(last)) => last.saturating_sub(first).max(0),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct DemoMetadata {
    pub size_bytes: usize,
    pub source: String,
    pub protocol: i32,
    pub fs_game: String,
    pub gametype: i32,
    pub server_name: Option<String>,
    pub duration_ms: i32,
    pub snapshot_count: usize,
    pub average_snapshot_rate: f32,
    pub recorded_client_num: Option<i32>,
    pub recorded_player_name: Option<String>,
    pub maps: Vec<DemoMapUsage>,
    pub players: Vec<DemoPlayerUsage>,
    pub rounds: Vec<DemoRoundStats>,
    pub duels: Vec<DemoDuelStats>,
    pub console: Vec<DemoConsoleEntry>,
    pub kill_count: usize,
}

#[derive(Debug, Clone)]
struct PlayerIdentity {
    name: String,
    model: String,
    team: i32,
}

#[derive(Debug, Clone)]
struct ActivePlayer {
    identity: PlayerIdentity,
    started_at: i32,
}

#[derive(Debug, Clone)]
struct PlayerAccum {
    identity: PlayerIdentity,
    duration_ms: i32,
    first_seen: i32,
    last_seen: i32,
    teams: BTreeSet<i32>,
}

#[derive(Debug, Clone)]
struct RoundAccum {
    map_name: String,
    start_at: i32,
    kills: usize,
    players: BTreeSet<String>,
    player_names: BTreeSet<String>,
    team_scores: Option<[i32; 2]>,
    scores: Vec<(String, i32)>,
    leader: Option<(String, i32)>,
}

#[derive(Debug, Clone)]
struct ActivePrivateDuel {
    pair: [i32; 2],
    players: Vec<String>,
    start_at: i32,
    winner: Option<String>,
}

#[derive(Debug, Clone)]
struct RawPrivateDuel {
    start_at: i32,
    end_at: i32,
    players: Vec<String>,
    winner: Option<String>,
}

fn clean_quake_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    let text = String::from_utf8_lossy(bytes);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    chars.next();
                    continue;
                }
                if next == '^' {
                    chars.next();
                    out.push('^');
                    continue;
                }
            }
        }
        if ch != '\u{19}' && ch != '\0' {
            out.push(ch);
        }
    }
    out.trim().to_owned()
}

fn player_identity(configstring: &[u8], fallback: usize) -> Option<PlayerIdentity> {
    if configstring.is_empty() {
        return None;
    }
    let name = info_value(configstring, b"n")
        .map(clean_quake_text)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("CLIENT {fallback}"));
    let model = info_value(configstring, b"model")
        .map(clean_quake_text)
        .unwrap_or_default();
    let team = info_value(configstring, b"t").map_or(0, atoi);
    Some(PlayerIdentity { name, model, team })
}

fn player_key(identity: &PlayerIdentity) -> String {
    format!("{}\\0{}", identity.name.to_ascii_lowercase(), identity.model.to_ascii_lowercase())
}

fn close_active_player(
    slot: usize,
    at: i32,
    active: &mut [Option<ActivePlayer>],
    players: &mut BTreeMap<String, PlayerAccum>,
) {
    let Some(current) = active.get_mut(slot).and_then(Option::take) else { return; };
    let key = player_key(&current.identity);
    let elapsed = at.saturating_sub(current.started_at).max(0);
    let entry = players.entry(key).or_insert_with(|| PlayerAccum {
        identity: current.identity.clone(),
        duration_ms: 0,
        first_seen: current.started_at,
        last_seen: at,
        teams: BTreeSet::new(),
    });
    entry.duration_ms = entry.duration_ms.saturating_add(elapsed);
    entry.first_seen = entry.first_seen.min(current.started_at);
    entry.last_seen = entry.last_seen.max(at);
    entry.teams.insert(current.identity.team);
}

fn update_player_slot(
    slot: usize,
    configstring: &[u8],
    at: i32,
    active: &mut [Option<ActivePlayer>],
    players: &mut BTreeMap<String, PlayerAccum>,
) {
    let next = player_identity(configstring, slot);
    let unchanged = match (active.get(slot).and_then(Option::as_ref), next.as_ref()) {
        (Some(old), Some(new)) => player_key(&old.identity) == player_key(new) && old.identity.team == new.team,
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        return;
    }
    close_active_player(slot, at, active, players);
    if let Some(identity) = next {
        active[slot] = Some(ActivePlayer { identity, started_at: at });
    }
}

fn client_team(configstrings: &BTreeMap<u16, Vec<u8>>, client_num: i32) -> i32 {
    let Ok(client) = u16::try_from(client_num) else { return 0; };
    let Some(index) = CS_PLAYERS.checked_add(client) else { return 0; };
    configstrings
        .get(&index)
        .and_then(|info| info_value(info, b"t"))
        .map_or(0, atoi)
}

fn client_name(configstrings: &BTreeMap<u16, Vec<u8>>, client: i32) -> String {
    u16::try_from(client)
        .ok()
        .and_then(|client| CS_PLAYERS.checked_add(client))
        .and_then(|index| configstrings.get(&index))
        .and_then(|info| info_value(info, b"n"))
        .map(clean_quake_text)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("CLIENT {client}"))
}

fn parse_scores(args: &[Vec<u8>], configstrings: &BTreeMap<u16, Vec<u8>>) -> (Option<[i32; 2]>, Vec<(String, i32)>) {
    if args.len() < 4 {
        return (None, Vec::new());
    }
    let count = atoi(&args[1]).clamp(0, 32) as usize;
    let team_scores = Some([atoi(&args[2]), atoi(&args[3])]);
    const SCORE_OFFSET: usize = 14;
    let mut scores = Vec::with_capacity(count);
    for row in 0..count {
        let base = 4 + row * SCORE_OFFSET;
        if args.len() < base + SCORE_OFFSET {
            break;
        }
        let client = atoi(&args[base]).clamp(0, 31);
        let score = atoi(&args[base + 1]);
        scores.push((client_name(configstrings, client), score));
    }
    (team_scores, scores)
}

fn console_text(args: &[Vec<u8>], start: usize) -> String {
    clean_quake_text(&args.get(start..).unwrap_or_default().join(&b' '))
}

/// Parse metadata only for the explicitly selected demo. This deliberately does
/// not pre-index neighboring demos; the menu calls it on demand and caches the result.
pub(super) fn index_demo_metadata(
    base: &Path,
    game: Option<&Path>,
    demo_name: &str,
) -> Result<DemoMetadata, String> {
    const MAX_DEMO_FILE_BYTES: usize = 512 * 1024 * 1024;
    let qpath = format!("demos/{demo_name}.dm_26");
    let mut assets = AssetSearchPath::open_game(base, game).map_err(|error| error.to_string())?;
    let asset = assets
        .read(&qpath, MAX_DEMO_FILE_BYTES)
        .map_err(|error| format!("{qpath}: {error}"))?
        .ok_or_else(|| format!("{qpath}: not found"))?;
    let size_bytes = asset.bytes.len();
    let source = asset.source.display().to_string();
    scan_demo_bytes(&asset.bytes, size_bytes, source).map(|(_, metadata)| metadata)
}

pub(super) fn build_demo_index(bytes: &[u8]) -> Result<DemoIndex, String> {
    scan_demo_bytes(bytes, bytes.len(), String::new()).map(|(index, _)| index)
}

fn scan_demo_bytes(
    bytes: &[u8],
    size_bytes: usize,
    source: String,
) -> Result<(DemoIndex, DemoMetadata), String> {
    const SNAPFLAG_NOT_ACTIVE: u8 = 1 << 1;
    const EV_EVENT_BITS: i32 = 0x300;
    const EV_OBITUARY: i32 = 93;
    const MAX_GENTITIES: usize = 1024;

    let mut reader = DemoReader::new(Cursor::new(bytes));
    let mut decoder = ServerMessageDecoder::new();
    let mut configstrings = BTreeMap::<u16, Vec<u8>>::new();
    let mut big_config = BigConfigString::default();
    let mut pending_commands = Vec::<Vec<u8>>::new();
    let mut pending_roster_refresh = false;
    let mut pending_map = None::<String>;
    let mut setgame = Vec::<u8>::new();
    let mut first_active = None::<i32>;
    let mut last_time = None::<i32>;
    let mut snapshot_count = 0usize;
    let mut recorded_client_num = None::<i32>;
    let mut recorded_player_name = None::<String>;
    let mut active_players = vec![None::<ActivePlayer>; 32];
    let mut player_accum = BTreeMap::<String, PlayerAccum>::new();
    let mut maps = Vec::<(String, i32, i32)>::new();
    let mut current_map = None::<(String, i32)>;
    let mut rounds = Vec::<DemoRoundStats>::new();
    let mut round = None::<RoundAccum>;
    let mut private_duel = None::<ActivePrivateDuel>;
    let mut private_duels = Vec::<RawPrivateDuel>::new();
    let mut console = Vec::<(i32, DemoConsoleKind, String)>::new();
    let mut transient_notices = Vec::<(i32, DemoConsoleKind, String)>::new();
    let mut kill_count = 0usize;
    let mut kill_markers = Vec::<DemoKillMarker>::new();
    let mut previous_event = [0i32; MAX_GENTITIES];
    let mut previous_present = [false; MAX_GENTITIES];

    let close_round = |at: i32, round: &mut Option<RoundAccum>, rounds: &mut Vec<DemoRoundStats>| {
        let Some(done) = round.take() else { return; };
        let duration_ms = at.saturating_sub(done.start_at).max(0);
        if duration_ms == 0 && done.kills == 0 && done.team_scores.is_none() && done.leader.is_none() {
            return;
        }
        rounds.push(DemoRoundStats {
            index: rounds.len() + 1,
            map_name: done.map_name,
            start_ms: done.start_at,
            duration_ms,
            kills: done.kills,
            players: done.players.len(),
            player_names: done.player_names.into_iter().collect(),
            team_scores: done.team_scores,
            scores: done.scores,
            leader: done.leader,
        });
    };

    while let Some(record) = reader.next_record().map_err(|error| format!("demo metadata framing error: {error}"))? {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .map_err(|error| format!("demo metadata protocol error at sequence {}: {error}", record.sequence))?;
        for event in packet.events {
            match event {
                ServerMessageEvent::Gamestate { client_number, .. } => {
                    configstrings = decoder.configstrings.clone();
                    recorded_client_num.get_or_insert(client_number);
                    pending_roster_refresh = true;
                    pending_map = decoder.map_name();
                }
                ServerMessageEvent::ServerCommand(command) => pending_commands.push(command.text),
                ServerMessageEvent::SetGame(game) => setgame = game,
                ServerMessageEvent::MapChange => {
                    // The following gamestate/snapshot provides the authoritative map name/time.
                }
                ServerMessageEvent::Snapshot { .. } => {
                    let Some(snapshot) = decoder.latest_snapshot().cloned() else { continue; };
                    if snapshot.snap_flags & SNAPFLAG_NOT_ACTIVE != 0 {
                        continue;
                    }
                    let server_time = snapshot.server_time;
                    if first_active.is_none() {
                        first_active = Some(server_time);
                    }
                    last_time = Some(server_time);
                    snapshot_count += 1;

                    if pending_roster_refresh {
                        for slot in 0..active_players.len() {
                            let index = CS_PLAYERS + slot as u16;
                            let value = configstrings.get(&index).cloned().unwrap_or_default();
                            update_player_slot(slot, &value, server_time, &mut active_players, &mut player_accum);
                        }
                        pending_roster_refresh = false;
                    }

                    let map_name = pending_map.take().or_else(|| {
                        configstrings.get(&0).and_then(|info| info_value(info, b"mapname"))
                            .map(clean_quake_text)
                    }).unwrap_or_else(|| "unknown".to_owned());
                    if let Some((old, started)) = current_map.clone() {
                        if old != map_name {
                            maps.push((old, started, server_time));
                            close_round(server_time, &mut round, &mut rounds);
                            current_map = Some((map_name.clone(), server_time));
                        }
                    } else {
                        current_map = Some((map_name.clone(), server_time));
                    }
                    if round.is_none() {
                        round = Some(RoundAccum {
                            map_name: map_name.clone(),
                            start_at: server_time,
                            kills: 0,
                            players: BTreeSet::new(),
                            player_names: BTreeSet::new(),
                            team_scores: None,
                            scores: Vec::new(),
                            leader: None,
                        });
                    }

                    let commands = std::mem::take(&mut pending_commands);
                    for raw in commands {
                        let command = match big_config.feed(&raw) {
                            BigConfigOutcome::PassThrough => raw,
                            BigConfigOutcome::Pending => continue,
                            BigConfigOutcome::Complete(command) => command,
                            BigConfigOutcome::Overflow => continue,
                        };
                        let args = tokenize(&command);
                        let Some(name) = args.first().map(Vec::as_slice) else { continue; };
                        if name.eq_ignore_ascii_case(b"cs") && args.len() >= 3 {
                            let index = atoi(&args[1]);
                            if let Ok(index) = u16::try_from(index) {
                                let value = args[2..].join(&b' ');
                                configstrings.insert(index, value.clone());
                                if index >= CS_PLAYERS && index < CS_PLAYERS + 32 {
                                    let slot = usize::from(index - CS_PLAYERS);
                                    update_player_slot(slot, &value, server_time, &mut active_players, &mut player_accum);
                                }
                            }
                            continue;
                        }
                        if name.eq_ignore_ascii_case(b"map_restart") {
                            console.push((server_time, DemoConsoleKind::Event, "map_restart".to_owned()));
                            close_round(server_time, &mut round, &mut rounds);
                            round = Some(RoundAccum {
                                map_name: current_map.as_ref().map(|v| v.0.clone()).unwrap_or_else(|| map_name.clone()),
                                start_at: server_time,
                                kills: 0,
                                players: BTreeSet::new(),
                                player_names: BTreeSet::new(),
                                team_scores: None,
                                scores: Vec::new(),
                                leader: None,
                            });
                            continue;
                        }
                        if name.eq_ignore_ascii_case(b"scores") {
                            let (team_scores, scores) = parse_scores(&args, &configstrings);
                            if let Some(active_round) = &mut round {
                                active_round.team_scores = team_scores;
                                active_round.leader = scores.iter().max_by_key(|(_, score)| *score).cloned();
                                active_round.scores = scores;
                            }
                            continue;
                        }
                        let (kind, start) = if name.eq_ignore_ascii_case(b"print") {
                            (Some(DemoConsoleKind::Print), 1)
                        } else if name.eq_ignore_ascii_case(b"chat") || name.eq_ignore_ascii_case(b"tchat") {
                            (Some(DemoConsoleKind::Chat), 1)
                        } else if name.eq_ignore_ascii_case(b"lchat") || name.eq_ignore_ascii_case(b"ltchat") {
                            (Some(DemoConsoleKind::Chat), 1)
                        } else if name.eq_ignore_ascii_case(b"cp") {
                            (Some(DemoConsoleKind::CenterPrint), 1)
                        } else {
                            (None, 0)
                        };
                        if let Some(kind) = kind {
                            let raw_text = String::from_utf8_lossy(&args.get(start..).unwrap_or_default().join(&b' '))
                                .trim_matches(|ch| ch == '\0' || ch == '\r' || ch == '\n')
                                .to_owned();
                            if matches!(kind, DemoConsoleKind::Chat | DemoConsoleKind::CenterPrint)
                                && !raw_text.is_empty()
                            {
                                transient_notices.push((server_time, kind, raw_text));
                            }
                            let text = console_text(&args, start);
                            if !text.is_empty() {
                                console.push((server_time, kind, text));
                            }
                        }
                    }

                    if let Some(active_round) = &mut round {
                        for player in active_players.iter().flatten().filter(|player| player.identity.team != 3) {
                            active_round.players.insert(player_key(&player.identity));
                            active_round.player_names.insert(player.identity.name.clone());
                        }
                    }
                    if recorded_player_name.is_none() {
                        if let Some(client) = recorded_client_num.and_then(|v| usize::try_from(v).ok()) {
                            recorded_player_name = active_players
                                .get(client)
                                .and_then(Option::as_ref)
                                .map(|player| player.identity.name.clone());
                        }
                    }

                    // The authoritative playerstate exposes private-duel state only for the
                    // recorded POV. Track those duels honestly as PRIVATE (POV); unseen
                    // third-party private duels cannot be reconstructed from a client demo.
                    let pov_client = snapshot.player_state.field_i32("clientNum").unwrap_or(-1);
                    let duel_in_progress = snapshot.player_state.field_i32("duelInProgress").unwrap_or(0) != 0;
                    let duel_opponent = snapshot.player_state.field_i32("duelIndex").unwrap_or(-1);
                    let requested_pair = duel_in_progress
                        .then_some([pov_client, duel_opponent])
                        .filter(|pair| pair[0] >= 0 && pair[1] >= 0 && pair[0] != pair[1]);
                    if private_duel.as_ref().map(|duel| duel.pair) != requested_pair {
                        if let Some(done) = private_duel.take() {
                            private_duels.push(RawPrivateDuel {
                                start_at: done.start_at,
                                end_at: server_time,
                                players: done.players,
                                winner: done.winner,
                            });
                        }
                        if let Some(pair) = requested_pair {
                            private_duel = Some(ActivePrivateDuel {
                                pair,
                                players: vec![client_name(&configstrings, pair[0]), client_name(&configstrings, pair[1])],
                                start_at: server_time,
                                winner: None,
                            });
                        }
                    }

                    let mut seen = [false; MAX_GENTITIES];
                    for state in &snapshot.entities {
                        let entity = usize::from(state.number);
                        if entity >= MAX_GENTITIES { continue; }
                        seen[entity] = true;
                        let entity_type = state.field_i32("eType").unwrap_or(0);
                        let event_only = entity_type > ET_EVENTS;
                        let raw_event = if event_only {
                            entity_type - ET_EVENTS
                        } else {
                            state.field_i32("event").unwrap_or(0)
                        };
                        let changed = if event_only { !previous_present[entity] } else { raw_event != previous_event[entity] };
                        previous_event[entity] = raw_event;
                        previous_present[entity] = true;
                        if changed && (raw_event & !EV_EVENT_BITS) == EV_OBITUARY {
                            let target = state.field_i32("otherEntityNum").unwrap_or(-1);
                            let attacker = state.field_i32("otherEntityNum2").unwrap_or(-1);
                            let means_of_death = state.field_i32("eventParm").unwrap_or(0);
                            let duplicate = kill_markers.iter().rev().take(16).any(|marker| {
                                marker.target == target
                                    && marker.attacker == attacker
                                    && marker.means_of_death == means_of_death
                                    && server_time.saturating_sub(marker.server_time).abs() <= 1000
                            });
                            if !duplicate {
                                kill_markers.push(DemoKillMarker {
                                    server_time,
                                    target,
                                    attacker,
                                    means_of_death,
                                    attacker_team: client_team(&configstrings, attacker),
                                });
                                kill_count += 1;
                                if let Some(active_round) = &mut round {
                                    active_round.kills += 1;
                                }
                                if let Some(duel) = &mut private_duel {
                                    let pair_match = (duel.pair[0] == target && duel.pair[1] == attacker)
                                        || (duel.pair[1] == target && duel.pair[0] == attacker);
                                    if pair_match && attacker >= 0 && attacker != target {
                                        duel.winner = Some(client_name(&configstrings, attacker));
                                    }
                                }
                            }
                        }
                    }
                    for entity in 0..MAX_GENTITIES {
                        if !seen[entity] {
                            previous_event[entity] = 0;
                            previous_present[entity] = false;
                        }
                    }
                }
                ServerMessageEvent::Nop | ServerMessageEvent::Download(_) => {}
            }
        }
    }

    let first = first_active.ok_or_else(|| "demo metadata found no active snapshots".to_owned())?;
    let last = last_time.unwrap_or(first);
    for slot in 0..active_players.len() {
        close_active_player(slot, last, &mut active_players, &mut player_accum);
    }
    if let Some((map_name, started)) = current_map.take() {
        maps.push((map_name, started, last));
    }
    close_round(last, &mut round, &mut rounds);
    if let Some(done) = private_duel.take() {
        private_duels.push(RawPrivateDuel {
            start_at: done.start_at,
            end_at: last,
            players: done.players,
            winner: done.winner,
        });
    }

    let serverinfo = configstrings.get(&0);
    let gametype = serverinfo
        .and_then(|info| info_value(info, b"g_gametype"))
        .map_or(0, atoi);
    let server_name = serverinfo
        .and_then(|info| info_value(info, b"sv_hostname"))
        .map(clean_quake_text)
        .filter(|name| !name.is_empty());
    let fs_game = configstrings
        .get(&jka_protocol::session::CS_SYSTEMINFO)
        .and_then(|info| info_value(info, b"fs_game"))
        .filter(|value| !value.is_empty())
        .map(clean_quake_text)
        .or_else(|| (!setgame.is_empty()).then(|| clean_quake_text(&setgame)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "base".to_owned());
    let duration_ms = last.saturating_sub(first).max(0);
    let average_snapshot_rate = if duration_ms > 0 {
        snapshot_count.saturating_sub(1) as f32 * 1000.0 / duration_ms as f32
    } else {
        0.0
    };

    let mut players: Vec<DemoPlayerUsage> = player_accum.into_values().map(|player| DemoPlayerUsage {
        name: player.identity.name,
        model: player.identity.model,
        duration_ms: player.duration_ms,
        first_seen_ms: player.first_seen.saturating_sub(first).max(0),
        last_seen_ms: player.last_seen.saturating_sub(first).max(0),
        teams: player.teams.into_iter().collect(),
    }).collect();
    players.sort_by(|a, b| b.duration_ms.cmp(&a.duration_ms).then_with(|| a.name.cmp(&b.name)));

    let mut map_usage = Vec::<DemoMapUsage>::new();
    for (map_name, start, end) in maps {
        let duration_ms = end.saturating_sub(start).max(0);
        if let Some(existing) = map_usage.iter_mut().find(|usage| usage.map_name == map_name) {
            existing.duration_ms = existing.duration_ms.saturating_add(duration_ms);
        } else {
            map_usage.push(DemoMapUsage { map_name, duration_ms });
        }
    }
    let maps = map_usage;
    for round in &mut rounds {
        round.start_ms = round.start_ms.saturating_sub(first).max(0);
    }
    let console: Vec<DemoConsoleEntry> = console.into_iter().map(|(at, kind, text)| DemoConsoleEntry {
        elapsed_ms: at.saturating_sub(first).max(0),
        kind,
        text,
    }).collect();

    let mut duels = Vec::<DemoDuelStats>::new();
    // GT_DUEL=3 / GT_POWERDUEL=4: map_restart-delimited game rounds are actual duel rounds.
    if matches!(gametype, 3 | 4) {
        for round in &rounds {
            if round.player_names.len() < 2 {
                continue;
            }
            let winner = round.scores.iter().max_by_key(|(_, score)| *score).and_then(|best| {
                let tied = round.scores.iter().filter(|(_, score)| score == &best.1).count() > 1;
                (!tied).then(|| best.0.clone())
            });
            let score_players: Vec<String> = round.scores.iter().map(|(name, _)| name.clone()).collect();
            let players = if score_players.len() >= 2 { score_players } else { round.player_names.clone() };
            duels.push(DemoDuelStats {
                index: duels.len() + 1,
                kind: DemoDuelKind::GameRound,
                start_ms: round.start_ms,
                duration_ms: round.duration_ms,
                players,
                winner,
                scores: round.scores.clone(),
            });
        }
    }
    for duel in private_duels.into_iter().filter(|_| !matches!(gametype, 3 | 4)) {
        let duration_ms = duel.end_at.saturating_sub(duel.start_at).max(0);
        if duration_ms <= 0 {
            continue;
        }
        duels.push(DemoDuelStats {
            index: duels.len() + 1,
            kind: DemoDuelKind::PrivatePov,
            start_ms: duel.start_at.saturating_sub(first).max(0),
            duration_ms,
            players: duel.players,
            winner: duel.winner,
            scores: Vec::new(),
        });
    }
    duels.sort_by_key(|duel| duel.start_ms);
    for (index, duel) in duels.iter_mut().enumerate() { duel.index = index + 1; }

    let transient_notices = transient_notices.into_iter().map(|(at, kind, text)| DemoConsoleEntry {
        elapsed_ms: at.saturating_sub(first).max(0),
        kind,
        text,
    }).collect();
    let index = DemoIndex {
        first_active_server_time: Some(first),
        last_server_time: Some(last),
        kill_markers,
        transient_notices,
    };

    Ok((index, DemoMetadata {
        size_bytes,
        source,
        protocol: 26,
        fs_game,
        gametype,
        server_name,
        duration_ms,
        snapshot_count,
        average_snapshot_rate,
        recorded_client_num,
        recorded_player_name,
        maps,
        players,
        rounds,
        duels,
        console,
        kill_count,
    }))
}
