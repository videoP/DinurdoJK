use jka_assets::pk3::AssetSearchPath;
use std::path::Path;

const LEVELSHOT_LIMIT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrontendPage {
    Main,
    Play,
    ServerBrowser,
    SoloGame,
    PlayDemo,
    Controls,
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
