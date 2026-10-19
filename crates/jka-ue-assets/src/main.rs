//! `djk-ue-import`: convert an exported Unreal project subset to native game assets.
use jka_ue_assets::importer::{self, ImportOptions};
use std::path::Path;

const USAGE: &str = "\
djk-ue-import import <export dir> <fresh output.pk3> [--scale 0.4] [--namespace models/ue] [--display-prefix Prefix_]
    Convert static/skeletal meshes, textures and material ports to a native PK3
    (plus <output>.report.json). --scale is UE centimetres to game units;
    --namespace is the quake-path folder every output lives in.
djk-ue-import audit-textures <export dir>/Textures <out.json>
    Classify exported PNGs (colour, normal, packed mask, ...) so
    tools/material-ports.py never uses a normal map as albedo.

See crates/jka-ue-assets/README.md for the export directory layout.";

fn main() {
    if let Err(error) = run() {
        eprintln!("djk-ue-import: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("import") => {
            let (mut positional, mut options) = (Vec::new(), ImportOptions::new(0.4));
            while let Some(arg) = args.next() {
                let mut value =
                    |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
                match arg.as_str() {
                    "--scale" => {
                        options.scale = value("--scale")?.parse().map_err(|_| "invalid --scale")?
                    }
                    "--namespace" => options.namespace = value("--namespace")?,
                    "--display-prefix" => options.display_prefix = value("--display-prefix")?,
                    flag if flag.starts_with("--") => {
                        return Err(format!("unknown option {flag}\n\n{USAGE}"))
                    }
                    _ => positional.push(arg),
                }
            }
            let [source, output] = positional.as_slice() else {
                return Err(format!("expected <export dir> <output.pk3>\n\n{USAGE}"));
            };
            importer::run(Path::new(source), Path::new(output), &options)
        }
        Some("audit-textures") => {
            let (Some(dir), Some(out), None) = (args.next(), args.next(), args.next()) else {
                return Err(format!("expected <Textures dir> <out.json>\n\n{USAGE}"));
            };
            jka_ue_assets::texture_audit::run(Path::new(&dir), Path::new(&out))
        }
        None | Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other}\n\n{USAGE}")),
    }
}
