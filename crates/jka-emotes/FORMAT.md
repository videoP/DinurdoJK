# `.djk_fnemote` format and recipes

## Container (version 1)

A deterministic ZIP with the extension `.djk_fnemote`. Inside a PK3 a bundle is
stored with ZIP *Store* (its members are already compressed) and read in place.

| Member | Purpose |
| --- | --- |
| `manifest.json` | `format: "djk_fnemote"`, `version: 1`, one emote (or one with `variants`), the conversion report and audio source mapping |
| `animation.gla` | target-compatible Ghoul2 skeleton and rigid animation |
| `animation.pose` | baked affine pose track that keeps animated scale: `DJKPOSE1` raw matrices, `DJKPOSE2` exact with float prediction + Zstandard, `DJKPOSE3` compact (checked error bound), `DJKPOSE4` local TRS pool plus exact affine residuals on the shipping grid |
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

## Lossless shipping-pose pool (`DJKPOSE4`)

`djk-fnemote pool-poses <input.pk3> <fresh output.pk3>` opts into this codec.
It streams a new archive directly, preserves every non-body-pose member, and
retains an original bundle whenever the candidate bundle is larger. A JSON
report beside the output records the selected clips, sizes, loading times and
validation. The input and existing output files are never overwritten.

This codec predicts skin matrices with compact parent-relative TRS records.
It uses the skin-matrix hierarchy directly: `local = inverse(parent_skin) *
child_skin`, with the root stored directly. These are **not** bind-space joint
transforms. The skeleton's parent array is embedded for decoding, including
parents that occur after children. No engine GLA format changes are needed.

Every predicted coefficient also has an integer affine residual. Reconstruction
uses the already corrected parent, rounds the predicted coefficient to the
`DJKPOSE3` grid, adds its residual, and performs the same integer-to-float divide
as `DJKPOSE3`. The encoder checks every input float bit against that grid and
the packager compares every float bit after the new ZIP is decoded. Therefore
the selected poses, including animated scale, shear and singular matrices,
are **bit exact relative to the existing decoded shipping poses**. The codec
does not recover detail already quantized by `DJKPOSE3`. Off-grid authoring
poses retain their original encoding.

All integers are little endian. The byte layout is:

| Field | Storage |
| --- | --- |
| Magic, frames, bones, pool count | `DJKPOSE4`, three `u32`s |
| Parents | `bones` signed `i32`s, `-1` for a root |
| Frame/bone references | 24-bit pool indices, delta predicted independently per bone, stored as three byte planes per bone across frames |
| Transform pool | 24-byte records, each byte lane delta predicted modulo 256 across records and stored as one byte plane |
| Residual stream lengths | `bones * 12` `u32`s, ordered bone / matrix row / column |
| Residual streams | signed ZigZag unsigned-LEB128 values, one value per frame in each stream |

Reference deltas wrap modulo `2^24`. Pool record byte 0 is the prediction mode.
Mode 0 contains four signed `i16` quaternion components (x,y,z,w), divided by
32767; three signed 24-bit translations divided by 1024; and three signed `i16`
scale offsets divided by 4096 and added to 1. Quaternion signs are canonicalized
by the encoder. The decoder uses the quaternion components directly without
normalizing them; the affine residual corrects their quantization as well.
Mode 1 predicts an absolute identity matrix and ignores the parent, allowing
the residual to retain a sample whose parent is singular or whose local TRS
is outside the record's range. Shear is retained in the affine residual in
either mode.

Prediction uses scalar `f64` quaternion matrix construction and matrix products
in the order implemented in `pose/pooled.rs`. Grid factors are 100000 for the
linear 3x3 coefficients and 1000 for translation. Rounding uses ties away from
zero. The corrected grid integer is converted to `f32` and divided by the
corresponding `f32` grid factor. Residuals are applied before predicting children,
so quaternion/scale quantization does not accumulate down the hierarchy.

The reader checks dimensions, a 128 MiB decoded-matrix limit, parent ranges and
cycles, pool references, prediction modes, stream lengths, varint overflow,
finite/bounded reconstructed coefficients and exact residual stream consumption.
`DJKPOSE1` through `DJKPOSE3` remain readable. Older clients reject `DJKPOSE4`
as an unknown pose header. The new reader expands to the same `PoseClip` matrix
array, so per-frame sampling and model retargeting are unchanged; this version
reduces installed storage without claiming a resident animation-memory reduction.
