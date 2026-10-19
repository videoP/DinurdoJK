# jka-emotes

Compile Unreal Engine animations (and the sound that goes with them) into
self-contained **emote bundles** the game can play on any player model, and read
those bundles back at runtime.

* **`djk-fnemote`** - the command line compiler / inspector.
* **`jka_emotes`** - the library the game uses to open bundles, plus the
  retargeter, pose and audio code the compiler is made of.

A bundle (`*.djk_fnemote`, a ZIP) holds one emote: the retargeted animation as a
Ghoul2 skeleton plus a baked pose track, mixed audio, and the source metadata it
was built from. Nothing is extracted to disk during playback. The name is
historical; nothing in the format is specific to one game, and any Unreal
project's `.psa` exports work (see [FORMAT.md](FORMAT.md) for every field).

Bundles are build artefacts. They are large, so they are **not** stored in this
repository (`.gitignore` excludes them); the game finds them in `base/emotes/`
or inside a PK3, and the [streaming seam](#where-bundles-live) below is how a
future download service plugs in.

```text
 .psa + .wav + metadata ──djk-fnemote compile──▶ EID_Name.djk_fnemote ──▶ base/emotes/   (or packed in a pk3)
 (your UE export)           (retarget + bake)       one file per emote        read by jka_emotes::Bundle
```

## Quick start

```sh
cargo build -p jka-emotes --release          # target/release/djk-fnemote

# 1. describe one emote (examples/emotes/dance-moves.recipe.json is a template)
# 2. compile it - never overwrites; point target_base at a Jedi Academy `base` dir
djk-fnemote compile my-emote.recipe.json base/emotes/EID_MyDance.djk_fnemote

# 3. check what came out
djk-fnemote inspect base/emotes/EID_MyDance.djk_fnemote
```

A minimal recipe: a PSA trimmed to 0-6.8 s, retargeted onto Kyle's rig, with one
music cue.

```json
{
  "id": "EID_MyDance", "name": "My Dance",
  "source_root": "export/MyDance",
  "animation": {
    "psa": "Animations/MyDance.psa",
    "target_base": "C:/Games/JediAcademy/GameData/base",
    "retarget": { "start_seconds": 0.0, "end_seconds": 6.8 }
  },
  "loop": false,
  "audio": [{ "at_ms": 0, "lane": "music", "audience": "both", "wav": "Audio/song.wav" }]
}
```

Other commands: `psa-info <file.psa>` (frames, fps, bones), `skeleton <base>`
(the target rig), `pack-library` / `compact-library` (below).

## Library

```rust
use glam::{Quat, Vec3};
use jka_emotes::{retarget, Bundle};
use serde_json::json;
use std::collections::BTreeMap;

// A one-frame animation on the stock humanoid rig, as a bundle on disk.
let gla = retarget::encode(&jka_assets::ghoul2::openjk_default_gla()?, 1, &[(Quat::IDENTITY, Vec3::ZERO)])?;
let manifest = json!({"format": "djk_fnemote", "version": 1, "emotes": [
    {"id": "EID_Test", "name": "Test", "gla": "animation.gla", "first_frame": 0, "end_frame": 1, "fps": 30}]});
let path = std::env::temp_dir().join(format!("doc-{}.djk_fnemote", std::process::id()));
jka_emotes::write_bundle(&path, &manifest, &BTreeMap::from([("animation.gla".into(), gla)]))?;

// Catalogue code reads only the directory and manifest...
let index = Bundle::open_index(&path)?;
assert_eq!(index.manifest["emotes"][0]["name"], "Test");
// ...playback validates the animation and audio, and reads members on demand.
let playable = index.validate_for_playback()?;
assert!(playable.read("animation.gla", 1 << 20)?.len() > 100);
# std::fs::remove_file(path)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`manifest::parse_manifest` turns a manifest into typed `Emote` values (variants,
audio events, loop points); `PoseClip` samples the baked pose track; `retarget`
and `psa` are the compiler's building blocks.

## Where bundles live

`Bundle` reads through a [`BundleSource`](src/source.rs), not a file path. Two
things ship today: a loose file, and a *stored* member of a larger archive, so a
whole library can be one PK3 that is read in place (`pack-library`).

To stream emotes later, implement `BundleSource` (a seekable reader per call and
a stable `key`) on top of ranged HTTP reads or a sparse on-disk cache, then call
`Bundle::from_source`. The design already fits that:

* **Cheap discovery.** `open_index` reads only the ZIP directory and manifest, a
  few KiB, so a catalogue of thousands never touches audio or poses.
* **Lazy, validated playback.** `validate_for_playback` fetches and checks what
  one emote needs; a changed remote file changes the cache identity
  (`runtime_prefix`), so stale audio is never reused.
* **Bounded everything.** 4096 members, 128 MiB per member, 512 MiB per bundle.

Deciding *what* is downloaded, where it is cached and how updates are announced
is deliberately left to the game's maintainers; this crate only guarantees that
no bundle has to exist on disk before it is played.

## Packaging

```sh
# one PK3 holding every bundle (stored, so the loader can seek into it) + shared assets
djk-fnemote pack-library base/emotes my-native-assets.pk3 out/library.pk3
# rewrite a directory of bundles smaller, without resampling poses or re-encoding audio
djk-fnemote compact-library base/emotes out/emotes-compact
```

Personal files such as `favorites.json` are never packed.

## Scope

* Retargeting is rest-relative affine: it keeps the target's bone lengths, strips
  root motion, keeps animated scale in the baked pose track and grounds the feet
  from the source's contact heights. It is a good starting point, not contact IK:
  preview every conversion on a real model.
* Sound graphs are baked to PCM offline; the runtime plays ordinary mono audio.
  Pitch/gain envelopes, delays and loops are handled by the compiler.
* Props, particle effects and the batch tooling for whole libraries are separate
  follow-ups; this crate's manifest already has room for them (`effects`,
  `presentation`) and ignores what it does not know.

```sh
cargo test -p jka-emotes     # bundle, pose, retarget, audio and packaging tests
```
