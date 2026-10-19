# Emotes

The client plays **local, cosmetic emotes** from compiled bundles
(`*.djk_fnemote`). Playback is presentation only: nothing is sent to the server,
other players do not see it, and moving or attacking stops it.

The bundles are not part of this repository (they are large). Build your own with
[`djk-fnemote`](../../crates/jka-emotes/README.md), or install a library someone
gives you. With none installed the emote UI simply shows an empty library.

## Installing

Put bundles in the game's active `base` directory, either loose under
`emotes/` or packed (one or many PK3s, bundles stored uncompressed so they can be
read in place):

```
GameData/base/emotes/EID_DanceMoves.djk_fnemote      loose
GameData/base/my-emotes.pk3                          packed (djk-fnemote pack-library)
```

`base/emotes/` in the source tree is only this readme and an ignore rule; generated
bundles and libraries are never committed. Props and particle effects live in an
optional native-assets PK3 next to the bundles (`models/fnemote/...`, see
[EFFECTS.md](../../crates/jka-emotes/EFFECTS.md)).

Keep `emotes/favorites.json` writable: it holds your starred emotes, categories and
chosen variations, and is never packaged.

## Using them

* **Profile → Emotes** lists the library: search, preview on your player model with
  sound, pick a variation, choose a category, star favorites, change the backdrop
  (an installed map's levelshot).
* **The `/emote` wheel**: hold the key bound to `emote` (set it under *Controls →
  Other → Emote Wheel*, or `bind B emote`), left-click **LIBRARY**, then
  left-click a starred emote. Right-click goes back; releasing the key cancels.
* **Console**: `djk_emote list`, `djk_emote 12`, `djk_emote EID_DanceMoves`,
  `djk_emote stop`, `djk_emote refresh` (rescan after installing).

| Setting | Meaning |
| --- | --- |
| `cg_emoteCubic 1` | cubic interpolation of body, props and ground contact at render rate (smooth above the 30 Hz source rate) |
| `cg_emoteEffects 1` | draw baked particle effects |
| `s_emoteMusicVolume 0.5` | emote music level, independent of effects and map music |

All three are also in the Settings menu (Audio, Models).

## Offline QA gallery (developers)

`emote_gallery <first> [count] [samples] | ids <EID,...> [samples] | stop` renders emotes
offline on the isolated Profile stage (no map) from four angles at scrubbed times, using the
normal screenshot path, and writes a `report.json` (missing props, track mismatches, audio
status, effect counts). `DJK_HEADLESS=1` keeps the window hidden and the process at
below-normal priority. `scripts/emote-gallery.sh` (Git Bash on Windows) runs several instances
in parallel from a test base folder and builds one contact sheet per emote.

## How it fits together (for contributors)

| Where | What |
| --- | --- |
| `crates/jka-client/src/emotes.rs` | catalogue (built on a background worker, shared by `Arc`), playback clock, prepared-asset caches (64 MiB each, favorites kept) |
| `app/emote_wheel.rs`, `egui_emote_radial.rs`, `egui_menu/profile_emotes.rs` | favorites, categories and variants; the wheel tier; the Profile tab |
| `audio.rs`, `cgame/sound_presenter.rs` | emote audio lanes (music / foley), sample-accurate loops, bounded decode cache |
| `cgame/player_presenter/{presentation,emote_props,emote_effects}.rs` | the pose track drives the viewer's player; props and baked particles follow the final pose |
| `cgame/entity_presenter.rs`, `fx/draw.rs` | prop MD3 / skinned props and particle draws |

Threading: nothing reads a bundle on the game thread. The catalogue scan, favorite
warm-up and cold-selection preparation run on bounded workers; a stale selection
never starts after a newer one or a stop. A damaged or duplicate bundle is logged
and skipped, never fatal to the rest of the library.

**Where a streaming store would plug in:** `emotes::read_catalog` / `read_bundle`
find bundles through the game's asset search path and open each one with
`jka_emotes::Bundle::open_located_index`. Replace that lookup with your own
`jka_emotes::BundleSource` (see the
[streaming note](../../crates/jka-emotes/README.md#where-bundles-live)) and the
rest of the client is unchanged: the catalogue only reads the ZIP directory and
manifest, and playback validates lazily.
