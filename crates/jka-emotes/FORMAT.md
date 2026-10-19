# `.djk_fnemote` format and recipes

## Container (version 1)

A deterministic ZIP with the extension `.djk_fnemote`. Inside a PK3 a bundle is
stored with ZIP *Store* (its members are already compressed) and read in place.

| Member | Purpose |
| --- | --- |
| `manifest.json` | `format: "djk_fnemote"`, `version: 1`, one emote (or one with `variants`), the conversion report and audio source mapping |
| `animation.gla` | target-compatible Ghoul2 skeleton and rigid animation |
| `animation.pose` | baked affine pose track that keeps animated scale: `DJKPOSE1` raw matrices, `DJKPOSE2` exact with float prediction + Zstandard, `DJKPOSE3` compact (checked error bound) |
| `audio/NNNN.wav`, `audio/mix-NNNN.flac` | selected WAVs and rendered sound graphs, de-duplicated |
| `effects/*.fxt`, `props/...` | optional baked particle tracks and prop tracks |
| `metadata/recipe.json`, `metadata/source/...` | the recipe and extracted source JSON, for auditing |
| `source/animation.psa`, `source/audio/...` | optional originals |

The reader checks, before a bundle is accepted: member names (no traversal, no
duplicates, no directories or symlinks), the format/version, at most 4096 members,
128 MiB per member, 512 MiB total, GLA bounds and parent cycles, pose dimensions
against the GLA, audio decoded size and loop points, and every manifest
reference. A reader's cache identity (`Bundle::runtime_prefix`) hashes every
member's CRC and size, so a refreshed bundle can never reuse stale data.

### Manifest

```json
{"format": "djk_fnemote", "version": 1, "target_model": "models/players/kyle/model.glm",
 "emotes": [{
   "id": "EID_MyDance", "name": "My Dance",
   "gla": "animation.gla", "pose": "animation.pose",
   "first_frame": 0, "end_frame": 204, "fps": 30.0, "loop": false,
   "loop_start_frame": 30,
   "audio": [ ...audio events... ],
   "variants": [ {"variant_name": "...", "first_frame": 0, "end_frame": 120, "loop": true, "audio": []} ]
 }],
 "conversion": { ...retarget report... }}
```

`end_frame` is exclusive. `variants` share one GLA/pose payload and are sampled
inside their own frame ranges. `loop_start_frame` makes the intro play once and
the rest repeat.

### Audio events

| Field | Meaning |
| --- | --- |
| `at_ms` | start, from the beginning of the trimmed animation |
| `lane` | `music` or `foley`; a new cue replaces the previous one on its lane |
| `audience` | `1p` (menu preview), `3p` (positional, in the world) or `both` |
| `wav` | member to play; omit for a *stop marker* for the lane |
| `repeat_on_loop` | repeat each animation cycle instead of firing once |
| `repeat_period_ms` | separate audio clock for looping music (needs `repeat_on_loop`, at most one hour) |
| `loop_start_sample` | in-track loop point; the intro plays once |

Gain/pitch/seek parameters are rejected at runtime; bake them into the audio (the
compiler's `mix` events do, see below).

## Recipe

One recipe compiles one emote. Relative paths resolve against the recipe file;
files under `source_root` may not escape it.

| Field | Meaning |
| --- | --- |
| `id`, `name` | identifier and display name |
| `source_root` | folder holding the exported PSA, audio and metadata |
| `animation.psa` *or* `animation.gla` | a UE animation to retarget, or an already baked GLA (then `fps`, `first_frame`, `end_frame` are required) |
| `animation.variants` | composed montage variations (used by library tooling) |
| `animation.target_base` / `target_gla` | PSA only, exactly one: a game `base` directory (target model is read from it) or a loose GLA path |
| `animation.target_model` | GLM qpath, default `models/players/kyle/model.glm`; its rig and mesh calibrate retargeting and ground contact |
| `animation.retarget` | `start_seconds`, `end_seconds`, `basis_xyzw`, `translation_scale`, `bone_map` (target bone to source bone), `ground_contact`, `origin_offset_z`, `finger_extension_degrees` |
| `loop` | whether the emote loops |
| `audio` | audio events; `wav` paths are relative to `source_root` |
| `include_source_psa`, `include_compressed_sources`, `include_all_audio` | archive originals in the bundle |

Rendered audio instead of a plain file: an event may carry
`"mix": {"duration_seconds": 9, "voices": [{"wav": "Audio/a.wav", "at_seconds": 0.2,
"gain": 0.8, "pitch": 1.1, "looping": true, "stop_seconds": 8.9}]}`; the compiler
renders it to a mono FLAC member and rewrites the event to `wav`.

Additive clips need the source skeleton's reference pose: point `DJK_UE_REFPOSE`
at a JSON file `{"skeleton": {bone: {"t": [x,y,z], "r": [x,y,z,w], "s": [x,y,z]}}}`
exported from your game's `USkeleton`.

## Coordinates

PSA data is converted from Unreal's left-handed Z-up basis to right-handed for
both bone references and keys. The retargeter transfers rest-relative motion,
derives facing and a translation scale from anatomical landmarks, strips the
source root trajectory, retains pelvis displacement and animated scale, and
restores the target's native root height. GLA cannot store animated scale, which
is why PSA bundles also carry the baked pose track; the GLA stays as the
skeleton compatibility check and rigid fallback.
