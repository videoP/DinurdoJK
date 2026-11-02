//! JK2 demo diagnostics use their own decoder and installation root. They never
//! mount JK2 assets in an active JKA session or invoke JKA native prediction.

use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

pub(crate) fn resolve_demo(root: Option<&Path>, name: &str) -> Result<PathBuf, String> {
    let given = Path::new(name);
    let candidates = if given.is_absolute() {
        vec![given.to_path_buf()]
    } else {
        if given
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("Use a demo name under fs_jk2path/base/demos, or an absolute path".into());
        }
        let base = root
            .ok_or("Set fs_jk2path to your JK2 GameData directory first")?
            .join("base");
        vec![
            base.join("demos").join(given),
            base.join("jk2demos").join(given),
        ]
    };
    if given.extension().is_some_and(|ext| {
        !ext.to_str().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("dm_15") || ext.eq_ignore_ascii_case("dm_16")
        })
    }) {
        return Err("Expected a JK2 .dm_15 or .dm_16 demo".into());
    }
    let mut matches = Vec::new();
    for path in candidates {
        let paths = if path.extension().is_none() {
            vec![path.with_extension("dm_15"), path.with_extension("dm_16")]
        } else {
            vec![path]
        };
        matches.extend(paths.into_iter().filter(|p| p.is_file()));
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(format!(
            "No JK2 demo found for {name}; checked base/demos and base/jk2demos"
        )),
        _ => Err("Multiple JK2 demos match; specify the extension or absolute path".into()),
    }
}

pub(crate) fn inspect(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    jka_protocol::jk2::demo::inspect(BufReader::new(file))
        .map(|summary| summary.report())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// A headless entry point in the normal game executable for reproducible user
/// tests. Run before updater/window/GPU/audio startup; print to the caller's
/// stdout so PowerShell redirection and exit codes work normally.
pub(crate) fn cli_entry() -> Option<std::process::ExitCode> {
    let mut args = std::env::args().skip(1);
    let flag = args.next();
    if !matches!(
        flag.as_deref(),
        Some("--jk2-demo-info" | "--jk2-presentation-info" | "--jk2-pmove-test")
    ) {
        return None;
    }
    let result = (|| {
        if flag.as_deref() == Some("--jk2-pmove-test") {
            let demo = args.next();
            if args.next().is_some() {
                return Err(
                    "Usage: DinurdoJK --jk2-pmove-test [demo name or absolute path]".into(),
                );
            }
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let base = exe
                .parent()
                .ok_or("Cannot locate executable directory")?
                .join("base");
            let mut roots = crate::filesystem_paths::load(&base);
            if roots.jk2path.is_none() {
                return Err("Set fs_jk2path to your JK2 GameData directory first".into());
            }
            let demo = demo
                .map(|name| resolve_demo(roots.jk2path.as_deref(), &name))
                .transpose()?;
            roots.active_jk2 = Some(2);
            jka_assets::pk3::configure_filesystem_roots(&base, roots);
            let mut assets =
                jka_assets::pk3::AssetSearchPath::open_game(&base, None).map_err(|e| e.to_string())?;
            let asset = assets
                .read("models/players/_humanoid/animation.cfg", 39998)
                .map_err(|e| e.to_string())?
                .ok_or("JK2 animation.cfg missing under fs_jk2path")?;
            let mut report = jka_movement::jk2::diagnostics::run(&asset.bytes)?;
            if let Some(demo) = demo {
                report.push_str(&format!(
                    "\n{}",
                    verify_native_snapshots(&demo, &asset.bytes)?
                ));
            }
            return Ok(format!(
                "Animation data: {} ({} bytes)\n{report}",
                asset.source.display(),
                asset.bytes.len()
            ));
        }
        let name = args
            .next()
            .ok_or("Usage: DinurdoJK --jk2-demo-info <demo name or absolute path>")?;
        if args.next().is_some() {
            return Err("Usage: DinurdoJK --jk2-demo-info <demo name or absolute path>".to_owned());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let base = exe
            .parent()
            .ok_or("Cannot locate executable directory")?
            .join("base");
        let roots = crate::filesystem_paths::load(&base);
        let path = resolve_demo(roots.jk2path.as_deref(), &name)?;
        if flag.as_deref() == Some("--jk2-presentation-info") {
            use std::io::Read;
            let mut bytes = Vec::new();
            File::open(&path)
                .map_err(|e| e.to_string())?
                .take(512 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 512 * 1024 * 1024 {
                return Err("JK2 demo exceeds 512 MiB".into());
            }
            let converted = jka_protocol::jk2::presentation::convert_demo(&bytes)?;
            let count = jka_protocol::jk2::presentation::validate_conversion(&bytes, &converted)?;
            Ok(format!("JK2 {} presentation verified: {count} snapshots, {} bytes, {} unsupported event occurrences omitted. Camera/player poses and entity trajectories match the source.", converted.version.name(), converted.bytes.len(), converted.omitted_events))
        } else {
            inspect(&path)
        }
    })();
    Some(match result {
        Ok(report) => {
            std::println!("{report}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            std::eprintln!("JK2 diagnostic failed: {error}");
            std::process::ExitCode::FAILURE
        }
    })
}

fn verify_native_snapshots(path: &Path, animations: &[u8]) -> Result<String, String> {
    use jka_movement::jk2::{Player, State, Version};
    let mut player: Option<Player> = None;
    let mut previous_version = None;
    let mut checked = 0u64;
    let mut remapped = 0u64;
    jka_protocol::jk2::demo::inspect_with(
        BufReader::new(File::open(path).map_err(|e| e.to_string())?),
        |decoder, snapshot| {
            let result = (|| -> Result<(), String> {
                let version = match decoder.version().ok_or("Missing JK2 version")? {
                    jka_protocol::jk2::Version::V102 => Version::V102,
                    jka_protocol::jk2::Version::V104 => Version::V104,
                };
                let raw = &snapshot.player_state;
                let state = State {
                    version,
                    fields: raw.fields.iter().map(|(_, bits)| bits).collect(),
                    stats: raw.stats,
                    persistant: raw.persistant,
                    ammo: raw.ammo,
                    powerups: raw.powerups,
                };
                if previous_version != Some(version) {
                    player = Some(Player::new(version, animations, &state)?);
                    previous_version = Some(version);
                }
                let player = player.as_mut().unwrap();
                player.set_network(&state)?;
                let returned = player.network();
                for (i, (name, expected)) in raw.fields.iter().enumerate() {
                    if returned.fields[i] == expected {
                        continue;
                    }
                    // The pinned 1.02 animation maps deliberately collapse some
                    // unavailable clips. All other wire fields must roundtrip.
                    if version == Version::V102
                        && matches!(name, "legsAnim" | "torsoAnim" | "forceDodgeAnim")
                    {
                        remapped += 1;
                        continue;
                    }
                    return Err(format!(
                        "snapshot {} time {}: native field {name} changed {expected:#x} -> {:#x}",
                        snapshot.message_num, snapshot.server_time, returned.fields[i]
                    ));
                }
                if (
                    returned.stats,
                    returned.persistant,
                    returned.ammo,
                    returned.powerups,
                ) != (state.stats, state.persistant, state.ammo, state.powerups)
                {
                    return Err(format!(
                        "snapshot {}: native array roundtrip differs",
                        snapshot.message_num
                    ));
                }
                checked += 1;
                Ok(())
            })();
            result.map_err(std::io::Error::other)
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(format!("{}: {checked} real demo snapshots imported/exported through native JK2 state; {remapped} lossy 1.02 animation mappings. Demo usercmds are unavailable; these are state-conversion checks.", path.file_name().unwrap_or_default().to_string_lossy()))
}
