//! `djk-fnemote`: compile, inspect and package `.djk_fnemote` emote bundles.
use jka_emotes::compact::compact_library;
use jka_emotes::compiler::{compile, read, target_from_model, DEFAULT_TARGET_MODEL};
use jka_emotes::{Bundle, MAX_MEMBER_BYTES};
use serde_json::json;
use std::path::Path;

fn main() {
    if let Err(error) = run() {
        eprintln!("djk-fnemote: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let command = args.first().and_then(|s| s.to_str()).unwrap_or("help");
    match command {
        "pack-library" if (4..=5).contains(&args.len()) => {
            let native = if args[2] == "-" {
                None
            } else {
                Some(Path::new(&args[2]))
            };
            let overrides = args.get(4).map(Path::new);
            let count = jka_emotes::package::library_with_overrides(
                Path::new(&args[1]),
                overrides,
                native,
                Path::new(&args[3]),
            )?;
            println!(
                "Packed {count} emotes directly into {}",
                Path::new(&args[3]).display()
            );
            Ok(())
        }
        "compile" if args.len() == 3 => compile(Path::new(&args[1]), Path::new(&args[2])),
        "compact-library" if (3..=4).contains(&args.len()) => compact_library(
            Path::new(&args[1]),
            Path::new(&args[2]),
            args.get(3).map(Path::new),
        ),
        "inspect" if args.len() == 2 => {
            let bundle = Bundle::open(Path::new(&args[1]))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&bundle.manifest).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        "psa-info" if args.len() == 2 => {
            let psa = jka_emotes::psa::Psa::parse(&read(Path::new(&args[1]), MAX_MEMBER_BYTES)?)?;
            println!(
                "{}",
                json!({"frames":psa.frames,"fps":psa.fps,"bones":psa.bones.iter().map(|b| &b.name).collect::<Vec<_>>() })
            );
            Ok(())
        }
        "skeleton" if (2..=3).contains(&args.len()) => {
            let bytes = target_from_model(
                Path::new(&args[1]),
                args.get(2)
                    .and_then(|v| v.to_str())
                    .unwrap_or(DEFAULT_TARGET_MODEL),
            )?;
            let target = jka_assets::ghoul2::parse_gla(&bytes)?;
            println!("{}",serde_json::to_string_pretty(&json!({"bones":target.skeleton.iter().map(|b|json!({"name":b.name,"parent":b.parent,"base_pose":b.base_pose})).collect::<Vec<_>>() })).map_err(|e|e.to_string())?);
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err("invalid arguments; run djk-fnemote --help".into()),
    }
}

const USAGE: &str = "djk-fnemote: compile self-contained emote bundles

  compile <recipe.json> <output.djk_fnemote>
  inspect <bundle.djk_fnemote>                    print the manifest
  psa-info <animation.psa>                        frames, fps, bone names
  skeleton <GameData/base> [target model.glm]     bones of a target model's rig
  pack-library <emotes dir> <native.pk3 or -> <fresh output.pk3> [overrides dir]
  compact-library <input dir> <fresh output dir> [GameData/base to recalibrate]

See crates/jka-emotes/README.md. Outputs are never overwritten.";
