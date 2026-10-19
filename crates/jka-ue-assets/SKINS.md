# Unreal characters as player models

Any exported Unreal skeletal-mesh character (body, head, hat, ...) can be used as a
normal player model: it is rebound onto the native 72-bone JK2 humanoid rig at load
time, so **every JKA animation, saber, bolt, ragdoll and shadow works with no
character-specific code**. Models without a `source_model.json` are completely
unaffected.

```text
UE export ──ue-skin-package.py──▶ skin.pk3 ──base/──▶ player model "skin" (model menu, /model skin)
(skeletal meshes, materials,      models/players/<name>/{source_model.json, partN.skeletal.json, textures, shaders}
 textures; not included)
```

Skin packs are large (textures dominate), so they are **not stored in the repository**; a
pack is an ordinary PK3 the game finds in `base/` like any other player model. Fetching
packs on demand is left to the maintainers: nothing else has to change when a pack
appears in `base/`.

## Package one

```sh
python scripts/ue-skin-package.py my-export my_hero out/my_hero.pk3 \
    --title "My Hero" --icon icon.png SK_Body SK_Head=MI_Face SK_Hat
```

Each mesh is a `SkeletalMesh` from the export (see
[EXPORT-FORMAT.md](EXPORT-FORMAT.md)); `=A,B,-` after a mesh overrides its material
sections in order (`-` keeps, `off` hides). The first mesh supplies the shared
skeleton. Options:

| Option | Meaning |
| --- | --- |
| `--profile ue` (default) / `fortnite` | which **material parameter names** to read (`BaseColor`, `Normal`, emissive, ...); add a profile in the script for your project's master materials |
| `--bone-map map.json` | `{"source bone": "ValveBiped.Bip01_Pelvis"}` for skeletons that do not use the UE mannequin names (`pelvis`, `spine_01`, `upperarm_l`, ...). Bones matching nothing follow their nearest mapped ancestor |
| `--attach Mesh=bone,x,y,z,pitch,yaw,roll` | mount a rigid accessory (a helmet with only a root bone) on a socket |
| `--idle dir --native pk3` | looping character effects baked by `djk-fnemote bake-idle` |
| `--female`, `--title`, `--icon` | metadata |

Output per section: `<surface>.png` (colour), `<surface>_n.png` (tangent normal at **half
the source resolution**, blue rebuilt from red/green, specular mask in alpha) and, for
emissive or profile effects, a small multi-stage shader. Single-stage surfaces use the
normal-mapped Source lighting path (best with `r_entityAmbientLighting bsp_lightgrid`).

## The descriptor

`models/players/<name>/source_model.json` (at most 64 KiB):

```json
{"format": "ue_skeletal", "schema_version": 1, "female": false, "title": "My Hero",
 "parts": [{"name": "SK_Body", "mesh": "models/players/my_hero/part0.skeletal.json",
            "shaders": ["models/players/my_hero/part0_s0"], "normals": ["models/players/my_hero/part0_s0_n.png"]}],
 "bone_map": {"root": "ValveBiped.Bip01_Pelvis"},
 "idle_effects": []}
```

1-16 parts; a section with an empty shader is hidden. Everything the descriptor names must
live inside the package (paths are validated).

## How the fit works

`jka_ue_assets::character::character_rig_input` merges the parts into one skeleton by bone
name (a part whose bind pose differs is re-placed onto the body's), maps bones to the
humanoid, and hands format-neutral geometry (`RigGeometry`) to `jka_assets::source1_rig::build_rig`,
the same builder Source models use. `build_rig` returns diagnostics for the fit (RMS, yaw,
worst bone, rebind displacement, weight truncations).

## Limits

Skin weights are merged and truncated by the rig builder (counted in the diagnostics); no
cloth/physics assets, morph targets or per-pixel material graphs (materials are ported by
parameter). Characters whose proportions are far from the humanoid are reshaped visibly;
check the fit diagnostics.

```sh
python scripts/test-ue-skins.py     # packages a synthetic skeletal mesh and checks the PK3
cargo test -p jka-ue-assets         # bone mapping and the rig fit
```
