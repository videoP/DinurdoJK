# Using Unreal props in Radiant / q3map2 maps

An import (`djk-ue-import import`) is the format the *game* uses for runtime
props. Mappers need something different: stock id Tech 3 / Jedi Academy
conventions they can place as `misc_model` and compile with q3map2.
`scripts/ue-mapper-pack.py` converts one into the other, and
`scripts/pack-map.py` ships the finished map with only what it uses.

```
export dir ──djk-ue-import──▶ native.pk3 ──ue-mapper-pack.py──▶ mapping pack ──(Radiant, q3map2)──▶ map.bsp ──pack-map.py──▶ map.pk3
```

## 1. Make a mapping pack

```sh
djk-ue-import import my-export my-native.pk3 --namespace models/mygame
python scripts/ue-mapper-pack.py my-native.pk3 mygame my-mapping --pk3 my-mapping.pk3
```

Output (`mygame` is the pack name: lower-case letters, digits, `_`):

| Path | Content |
| --- | --- |
| `models/mygame/<asset>.md3` | readable names (`SM_Crate` → `crate.md3`), every surface points at `textures/mygame/<material>` |
| `textures/mygame/<material>.tga` | base colour with the authored tint folded in (`--jpg` stores opaque ones as JPEG) |
| `shaders/mygame.shader` | only for masked, translucent and additive materials, each with `qer_editorimage` |
| `models/mygame/README.txt` | model list with the source asset of each |

Opaque materials get no shader at all: an implicit shader (texture × lightmap)
is what lets q3map2 light props like brush faces.

## 2. Map with it

Put the pack (folder or `.pk3`) in the game's `base`, restart Radiant and place
props as `misc_model`. Compile as usual:

```sh
q3map2 -game ja -meta mymap.map
q3map2 -game ja -vis  mymap.map
q3map2 -game ja -light mymap.map
```

For collision put a `textures/system/clip` brush around a prop. q3map2's
`misc_model` autoclip (spawnflag 2) produces no clip with the JA gamepack and
is very slow on detailed meshes.

## 3. Ship the map

Props are baked into the BSP, so the finished map needs only the textures and
shaders it really uses:

```sh
python scripts/pack-map.py mymap.pk3 base/maps/mymap.bsp --from my-mapping
```

`pack-map.py` reads the BSP's shader lump (Q3 `IBSP` or JA `RBSP`), copies
each used texture and shader block from the `--from` sources and lists anything
it did not find (stock game content such as `textures/system/clip` is expected
there). The result is a normal self-contained map package.

## Limits

Engine-only material programs, animated or procedural Unreal materials and
PBR inputs are not part of the pack: those materials keep their base colour.
MD3 limits apply (63-character names, 4096 vertices per surface), which is why
asset names are shortened and de-duplicated.

## Test

```sh
python scripts/test-ue-mapping.py
```

builds a synthetic export, runs the whole chain including a fake BSP, and
checks the result.
