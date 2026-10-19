# jka-ue-assets

Convert meshes, textures and material parameters exported from **any Unreal
Engine project** (UE4 or UE5) into the game's native formats: MD3 for static
geometry, weighted `.djk_prop` for skinned geometry, and a quake shader script
for materials, all in one PK3.

The crate never reads `.uasset`/`.pak`/`.ucas` files and links no Epic code.
It consumes a plain **export directory** (JSON + PNG) that any exporter can
produce; see [EXPORT-FORMAT.md](EXPORT-FORMAT.md) for the exact layout. The
exporter itself is deliberately not part of this repository: how you obtain
the data (CUE4Parse, UModel, your own editor tooling) is up to you and your
game's licence.

```
 your exporter ──▶ export directory ──▶ djk-ue-import ──▶ models/<ns>/*.md3, textures, shaders (PK3)
 (not included)    JSON + PNG           (this crate)       + <output>.report.json
```

## Command line

Build: `cargo build -p jka-ue-assets --release` → `target/release/djk-ue-import`.

```sh
# 1. (optional) classify textures so a normal map is never used as albedo
djk-ue-import audit-textures my-export/Textures my-export/texture-classes.json
python crates/jka-ue-assets/tools/material-ports.py my-export

# 2. convert; 0.4 converts UE centimetres to game units, --namespace picks the output folder
djk-ue-import import my-export my-props.pk3 --scale 0.4 --namespace models/mygame
```

Drop `my-props.pk3` into `base/`. Every model, texture and shader it contains
lives under `models/mygame/` (shaders in `shaders/mygame_props.shader`), so two
imports with different namespaces never collide. `my-props.report.json` lists
every model and material with its conversion status and any rejected asset.

## Library

```rust
use jka_ue_assets::importer::{run, ImportOptions};

let options = ImportOptions { namespace: "models/mygame".into(), ..ImportOptions::new(0.4) };
run("my-export".as_ref(), "my-props.pk3".as_ref(), &options)?;
```

Lower-level building blocks are public too: `importer::md3` (one exported mesh
to MD3 bytes), `mesh::Mesh` (validated `.djk_prop` parser), `transform`
(UE→game handedness, reference skeleton matrices, FTransform composition),
`material::Program` (validated native expression program), `blueprint`
(Animation Blueprint slot router) and `programs::inventory` (what cooked
Blueprint/Niagara/material code a package carries, without executing it).

## Conventions

* **Coordinates.** UE is left-handed, Z-up, centimetres. Positions get
  `(x, -y, z) * scale`, normals `(x, -y, z)`, and triangle winding is reversed
  to compensate for the reflection.
* **MD3 limits.** A section is split into surfaces of at most 4096 vertices and
  8192 triangles. A mesh whose scaled extent exceeds the MD3 coordinate range is
  rejected and reported (choose a smaller `--scale`) rather than truncated.
* **Skinning.** `.djk_prop` keeps the four strongest influences, renormalised.
  Skinning uses the *mesh* reference-pose inverse bind, even when an
  animation's skeleton rest pose differs.
* **Materials.** Cooked shader maps are not editor graphs and are not
  interpreted. A material becomes a *port*: base-colour texture, tint, opacity,
  emissive pass, normal map, blend mode and two-sidedness. Anything else stays in
  the report as `unsupported_graph` and renders with a visible checker texture
  instead of silently looking "right".
* **Safety.** Every size is bounded (128 MiB per source file, 2 M vertices),
  identifiers must be 24-hex ids and namespaces are validated, so an untrusted
  export cannot write outside the PK3 or exhaust memory.

## Not included

Full PBR, world-position offset / vertex animation textures, Niagara,
Blueprint world behaviour and GPU resource execution are separate runtime
concerns (see `jka-ue-runtime`). Unity asset bundles are not supported.

## Testing

```sh
cargo test -p jka-ue-assets
```

`tests/import_export_dir.rs` builds a minimal export directory (one triangle,
one material, one texture) and runs the whole import, so it doubles as an
executable example of the input contract.
