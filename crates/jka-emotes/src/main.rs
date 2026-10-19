//! `djk-fnemote`: compile, inspect and package `.djk_fnemote` emote bundles.
use jka_emotes::compact::compact_library;
use jka_emotes::compiler::{compile, read, target_from_model, DEFAULT_TARGET_MODEL};
use jka_emotes::library::{
    batch, catalog, loop_audit, presentation_import, reconcile, reground, shipping,
};
use jka_emotes::{effect_preview, idle_effects, niagara_bake, program_audit};
use jka_emotes::{Bundle, MAX_MEMBER_BYTES};
use serde_json::json;
use std::{fs, path::Path};

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
        "import-assets" if (3..=4).contains(&args.len()) => {
            let scale = args
                .get(3)
                .and_then(|a| a.to_str())
                .unwrap_or("0.4")
                .parse()
                .map_err(|_| "invalid asset scale")?;
            import_assets(Path::new(&args[1]), Path::new(&args[2]), scale)
        }
        "bake-idle" if args.len() == 4 => idle_effects::run(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
        ),
        "audit-ue-programs" if args.len() == 3 => {
            program_audit::run(Path::new(&args[1]), Path::new(&args[2]))
        }
        "effect-preview" if args.len() == 5 => {
            let variant = args[2]
                .to_str()
                .and_then(|s| s.parse().ok())
                .ok_or("variant must be an index")?;
            effect_preview::run(
                Path::new(&args[1]),
                variant,
                Path::new(&args[3]),
                Path::new(&args[4]),
            )
        }
        "niagara-audit" if (3..=5).contains(&args.len()) => {
            let seconds = args
                .get(3)
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(3.0);
            niagara_bake::audit(
                Path::new(&args[1]),
                Path::new(&args[2]),
                seconds,
                args.get(4).and_then(|s| s.to_str()),
            )
        }
        "niagara-preview" if (4..=5).contains(&args.len()) => {
            let seconds = args
                .get(4)
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(3.0);
            niagara_bake::preview(
                Path::new(&args[1]),
                args[2].to_str().unwrap_or(""),
                Path::new(&args[3]),
                seconds,
            )
        }
        "loop-audit" if args.len() == 3 => {
            loop_audit::run(Path::new(&args[1]), Path::new(&args[2]))
        }
        "build-presentation" if args.len() == 7 => {
            let output = Path::new(&args[6]);
            let staging = output.with_extension("compiled-emotes");
            if output.exists() {
                return Err("shipping PK3 already exists".into());
            }
            let native = Path::new(&args[5]);
            presentation_import::run(
                Path::new(&args[1]),
                Path::new(&args[2]),
                Path::new(&args[3]),
                Path::new(&args[4]),
                &native.with_extension("report.json"),
                &staging,
            )?;
            let count = jka_emotes::package::library_with_overrides(
                Path::new(&args[4]),
                Some(&staging),
                Some(native),
                output,
            )?;
            println!(
                "Compiled and packed {count} emotes into {}",
                output.display()
            );
            Ok(())
        }
        "compile-presentation" if args.len() == 7 => presentation_import::run(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
            Path::new(&args[5]),
            Path::new(&args[6]),
        ),
        "catalog-index" if args.len() == 3 => {
            let index = catalog::index_source(Path::new(&args[1]))?;
            let output = Path::new(&args[2]).join("catalog-links.json");
            fs::write(
                &output,
                serde_json::to_vec_pretty(&index).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "Indexed {} proven aliases: {}",
                index["links"].as_object().map_or(0, |m| m.len()),
                output.display()
            );
            Ok(())
        }
        "reconcile-library" if (7..=8).contains(&args.len()) => reconcile::run(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
            Path::new(&args[5]),
            Path::new(&args[6]),
            args.get(7)
                .and_then(|a| a.to_str())
                .unwrap_or("4")
                .parse()
                .map_err(|_| "invalid jobs")?,
        ),
        "batch" if (4..=8).contains(&args.len()) => batch::run(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            args.get(4)
                .map(|v| v.to_str().unwrap_or("").parse::<usize>())
                .transpose()
                .map_err(|_| "jobs must be an integer")?
                .unwrap_or(2),
            args.get(5).and_then(|v| v.to_str()),
            args.get(6)
                .and_then(|v| v.to_str())
                .unwrap_or(DEFAULT_TARGET_MODEL),
            args.get(7).map(Path::new),
        ),
        "reground-library" if (6..=8).contains(&args.len()) => reground::run(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
            Path::new(&args[5]),
            args.get(6)
                .and_then(|a| a.to_str())
                .unwrap_or("4")
                .parse()
                .map_err(|_| "invalid jobs")?,
            args.get(7).and_then(|a| a.to_str()),
        ),
        "ship-library" if (4..=6).contains(&args.len()) => {
            let output = Path::new(&args[3]);
            let packed = output
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pk3"));
            if packed && output.exists() {
                return Err("shipping PK3 already exists".into());
            }
            let stage = if packed {
                output.with_extension("compiled-emotes")
            } else {
                output.to_owned()
            };
            shipping::run(
                Path::new(&args[1]),
                Path::new(&args[2]),
                &stage,
                args.get(4).and_then(|a| a.to_str()).unwrap_or("small"),
                args.get(5)
                    .and_then(|a| a.to_str())
                    .unwrap_or("4")
                    .parse()
                    .map_err(|_| "invalid jobs")?,
            )?;
            if packed {
                jka_emotes::package::library(&stage, None, output)?;
            }
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

Library tooling for Fortnite-style source exports (see crates/jka-emotes/LIBRARY.md):
  batch <export> <GameData/base> <out/emotes> [jobs] [IDs or -] [target model.glm] [supplement dir]
  ship-library <export> <authoring/emotes> <out> [small|lossless|tiny] [jobs]   (.pk3 out packs it)
  reground-library <export> <authoring> <shipping> <GameData/base> <out> [jobs] [IDs]
  reconcile-library <export> <supplement> <authoring> <shipping> <GameData/base> <out> [jobs]
  compile-presentation <export> <metadata> <authoring dirs;...> <shipping> <native report.json> <out>
  build-presentation <export> <metadata> <authoring dirs;...> <shipping> <native.pk3> <out.pk3>
  catalog-index <export> <out dir>                writes catalog-links.json
  loop-audit <emotes dir> <out.json>

Native assets and effects (see crates/jka-emotes/EFFECTS.md):
  import-assets <UE export dir> <fresh output.pk3> [cm to game units = 0.4]
  bake-idle <native report.json> <request.json> <out dir>   character idle effects (see SKINS.md)
  audit-ue-programs <UE export dir> <out.json>    which Blueprint/Niagara programs decode
  niagara-audit <UE export dir> <out.json> [seconds = 3] [name filter]
  niagara-preview <UE export dir> <name filter> <out.png> [seconds = 3]
  effect-preview <bundle> <variant> <fx-native dir> <out.png>

See crates/jka-emotes/README.md. Outputs are never overwritten.";

/// Import props with the emote runtime's namespace and naming (`models/fnemote/`).
/// For any other project use `djk-ue-import --namespace`.
fn import_assets(source: &Path, output: &Path, scale: f32) -> Result<(), String> {
    let options = jka_ue_assets::importer::ImportOptions {
        namespace: jka_emotes::NATIVE_NAMESPACE.into(),
        display_prefix: "Emote_Prop_".into(),
        ..jka_ue_assets::importer::ImportOptions::new(scale)
    };
    jka_ue_assets::importer::run(source, output, &options)
}
