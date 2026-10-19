# exporter: Unreal asset export (CUE4Parse)

`FnAssetExport` reads a game's packed Unreal files with [CUE4Parse](https://github.com/FabianFG/CUE4Parse)
and writes the **export directory** that [`djk-ue-import`](../crates/jka-ue-assets/README.md) converts into
game assets (layout: [EXPORT-FORMAT.md](../crates/jka-ue-assets/EXPORT-FORMAT.md)).

```text
your game's .pak / .utoc  ──FnAssetExport (this folder)──▶  export dir  ──djk-ue-import──▶  PK3
```

It is a thin layer over CUE4Parse: CUE4Parse parses, this tool decides *what* to export and in which layout.
CUE4Parse is included as a **git submodule pinned to one commit** (Apache-2.0), so nothing of it is copied here.
It works for any Unreal game (`project` config below) and has built-in defaults for Fortnite.

## What it does
Starting from the assets you name (seeds), it follows their dependencies (up to a depth/object limit) and writes:

| Folder / file | Content |
| --- | --- |
| `Meshes/<id>.json`, `.skeletal.json` | static / skeletal meshes (first LOD), with per-section material ids |
| `Textures/<id>.png` | textures (BC/ETC decoded) |
| `Animations/<id>.psa` | animation sequences (ActorX) |
| `materials.json` | per material: blend mode, shading model, texture/colour/scalar/switch parameters |
| `Objects/<id>.json`, `Programs/<id>.json` | package objects (Niagara, Blueprint, materials) for the runtime crates |
| `Shaders/` | shader-library maps (can be turned off) |
| `report.json`, `dependencies.json` | every asset and its status; the seed list |

`<id>` is the first 24 hex digits of `SHA-256(lower-case asset path)`. Failures are recorded per asset, never fatal.

## Build
```sh
git clone --recurse-submodules <this repo>        # or: git submodule update --init exporter/CUE4Parse
dotnet build exporter/fn-assets/FnAssetExport.csproj -c Release -o target/ue-exporter
```
Needs the .NET 10 SDK. The CUE4Parse commit is pinned by the submodule; bump it deliberately
(`git -C exporter/CUE4Parse checkout <commit>`) and re-check the output.

## What you must provide (not included, never commit)
Under a folder you pass as `<exporter root>`, in `.data/`:
- the Oodle DLL (`oodle-data-shared.dll` on Windows),
- a `.usmap` **mappings** file for your game build (Fortnite: `Fortnite-<version>.usmap`),
- `aes.json`: `{"data": {"mainKey": "0x...", "dynamicKeys": [{"pakGuid": "...", "key": "0x..."}]}}`.

Use only a game you own and keys/mappings you obtained legitimately.

## Use
```text
FnAssetExport <exporter root> <game root> <dependencies.json> <fresh output dir> [max objects=12000] [max depth=64] [shader maps=true]
```
The output directory must not exist. `<game root>` is the game install (Fortnite: it contains `FortniteGame/Content/Paks`).

**1. List item paths** (to find what to export):
```sh
echo '{"list": "Cosmetics/Characters/"}' > list.json
dotnet target/ue-exporter/FnAssetExport.dll ~/exporter-data "C:/Program Files/Epic Games/Fortnite" list.json out/list   # writes out/list/listing.txt
```
**2. Export assets** (Fortnite defaults: `GAME_UE6_0`, `.data/Fortnite-42.30.usmap`, `.data/aes.json`):
```sh
echo '{"seeds": [{"asset": "/Game/Props/SM_Chair.SM_Chair"}]}' > deps.json
dotnet target/ue-exporter/FnAssetExport.dll ~/exporter-data "C:/Program Files/Epic Games/Fortnite" deps.json out/props
djk-ue-import import out/props props.pk3 --namespace models/mygame
```
**3. Any other Unreal game**: add a `project` block (see [`examples/generic-project.json`](examples/generic-project.json)):

| Key | Meaning |
| --- | --- |
| `pak_directory` | folder with the game's `.pak` / `.utoc` files |
| `game_version` | a CUE4Parse `EGame` name, e.g. `GAME_UE5_4`: **required**, it fixes the cooked layout |
| `mappings` | path to your `.usmap` (omit if property names are serialized) |
| `aes_file` | path to your `aes.json` (omit if unencrypted) |
| `on_demand_host` | optional streaming host for on-demand texture chunks (Fortnite keeps only small mips locally). `FN_ONDEMAND_HOST` / `FN_ONDEMAND_CACHE` env vars do the same; the cache lives outside the export folder |

Other top-level keys: `seeds` (assets to start from), `list` (listing mode), `shader_map_hashes`, and `project.export_shader_libraries` (default `true`).

## Limits
Materials are exported as parameters, not graphs; cooked shader code is retained only as authoring data. Unsupported or
unavailable assets are recorded with a status and skipped. Only the first LOD of a mesh is exported.
