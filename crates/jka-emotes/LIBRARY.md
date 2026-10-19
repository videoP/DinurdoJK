# Building a whole emote library

`djk-fnemote compile` turns one recipe into one bundle. The tooling in
`jka_emotes::library` turns a **source export of hundreds or thousands of items**
(the Fortnite layout) into a library, keeps it up to date, and packs it. None of
it is needed to *play* emotes, and its output is never committed: bundles and
libraries are build artefacts (see [the streaming note](README.md#where-bundles-live)).

The exporter that produces the source folders is **not part of this repository**;
this document only states what the tooling expects to find.

```text
export/<EID>/{ExportReport.json, Item.json, PlaybackMetadata.json,
              Animations/*.psa, Animations/Json/*.json, Audio/*.wav, Audio/AudioReport.json, Objects/...}
   │  batch                       one authoring bundle per item (montages -> variants, sound graphs mixed)
   ▼
authoring/*.djk_fnemote  + recipes/ + batch-report.json + audio-issues.json + catalog-links.json
   │  ship-library [small|lossless|tiny]    playback-only bundles (smaller audio/poses)
   ▼
shipping/*.djk_fnemote ──build-presentation──▶ props and effects baked in ──▶ library.pk3
```

## Commands

| Command | What it does |
| --- | --- |
| `batch <export> <base> <out> [jobs] [IDs\|-] [target.glm] [supplement]` | convert every item (or a comma-separated list of IDs) to authoring bundles; reuses matching recipes, never overwrites |
| `ship-library <export\|-> <authoring> <out> [small\|lossless\|tiny] [jobs]` | playback-only bundles; an output ending in `.pk3` is packed directly |
| `build-presentation <export> <metadata> <authoring dirs;...> <shipping> <native.pk3> <out.pk3>` | bake props and effects, then pack one PK3 |
| `compile-presentation ...` | the bake step alone (output stays a directory of bundles) |
| `reground-library <export> <authoring> <shipping> <base> <out> [jobs] [IDs]` | recalibrate ground contact for a new target model without changing audio or timelines |
| `reconcile-library <export> <supplement> <authoring> <shipping> <base> <out> [jobs]` | apply a supplemental export: rebuild what changed, copy the rest byte for byte |
| `catalog-index <export> <out dir>` | write `catalog-links.json` (which items are roles of one emote) |
| `loop-audit <emotes> <out.json>` | measure loop-seam jumps against neighbouring motion |

Shipping profiles: **small** keeps exact pose matrices with Vorbis audio
(48 kbps VBR); **tiny** additionally rounds matrices to a checked error bound
(coefficients 5.1e-6, translation 5.1e-4 game units, `shipping-report.json`
records the real values); **lossless** keeps rendered audio as FLAC. Every frame
and joint is retained in all of them.

## Example

```sh
# 1. authoring bundles for two items, retargeted onto Kyle
djk-fnemote batch export "GameData/base" out/authoring 2 EID_DanceMoves,EID_Floss
# 2. playback-size bundles, packed as one library the loader reads in place
djk-fnemote ship-library export out/authoring out/library.pk3 tiny 4
```

`ship-library -  <authoring> <out> tiny` re-packs an already shipped library
without the export (the export argument is unused then).

## What the tooling resolves

Source exports hold animation *tracks*, not complete performances: a performance
can be split into corrective tracks, gender alternatives, loops and connectors.
The tooling follows the source data instead of guessing:

* Montage section names are **case-insensitive** (`lOOP` reaches `Loop`);
  explicit random-section choices become separate playable variations with their
  own intros and loops; `RandomizeNextSection` notifies insert loop connectors
  (fixed loop counts exactly, ranged counts at the midpoint).
* Items named only by an ID are grouped under a named emote through item links and
  shared asset paths (`catalog-index`), never by ID prefix or fuzzy title.
  Audio is shared only for a proven shared montage with identical timing.
* Audio graphs (sound cues, wave players, mixers, delays, envelopes, loops,
  attenuation wrappers) are baked per audience and lane. Anything that cannot be
  resolved is recorded in `audio-issues.json` with its source retained; nothing is
  invented.
* Ground contact follows the composed source animation frame by frame, so hops
  stay airborne and planted poses stay planted (`reground-library` upgrades old
  libraries).

## Known limits

Gameplay-dependent behaviour (character parts, abilities, group
synchronisation) is not executed. Random connector destinations are baked in
source order into a repeatable cycle. Items whose animation packages the
exporter could not read are skipped and listed in `batch-report.json`.

## Opt-in checks

`tests/real_assets.rs` and `tests/presentation_assets.rs` run against *licensed
local data* and are `#[ignore]`d; each names the environment variables it needs
(`DJK_EMOTE_BASE`, `DJK_EMOTE_BUNDLE`, `DJK_PRESENTATION_DIR`, ...).
`scripts/verify-emote-pk3.py` and `scripts/verify-emote-presentation.py` check a
packed library byte for byte (every bundle exactly once, character motion and
audio unchanged by a presentation bake) without extracting anything.
