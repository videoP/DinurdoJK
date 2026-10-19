# Unreal / emotes integration

Integration date: 2026-10-19. Local branch: `integration/unreal-emotes-11-prs`.
Starting commit: `ac9caca` (`Updates`). The `main` branch remains at this commit.
No changes were pushed and no GitHub PR was merged remotely.

## Sources and order

PR metadata was read from GitHub's API sorted by creation time ascending.
Heads were fetched as `origin/pr/1` through `origin/pr/11`. Each stage has
its own local merge commit, preserving the original commits and ancestry.
PRs #1–#9 form a stack; #10 and #11 branch from #1. Git's ancestry tracking
prevents their shared commits from being applied again.

All creation times below are UTC on 2026-10-18.

| Order / PR | Created | Scope | Source head |
| --- | --- | --- | --- |
| [#1](https://github.com/videoP/DinurdoJK/pull/1) | 20:47:49 | Unreal mesh/material imports | `9ca7407` |
| [#2](https://github.com/videoP/DinurdoJK/pull/2) | 20:49:39 | Blueprint, Niagara, material runtime | `6d8c1ea` |
| [#3](https://github.com/videoP/DinurdoJK/pull/3) | 20:49:52 | Ghoul2 robustness | `cd94d0f` |
| [#4](https://github.com/videoP/DinurdoJK/pull/4) | 20:50:07 | Emote bundles and compiler | `20bb3ad` |
| [#5](https://github.com/videoP/DinurdoJK/pull/5) | 20:50:27 | Emote effects and props | `bf8b9ab` |
| [#6](https://github.com/videoP/DinurdoJK/pull/6) | 20:50:45 | Whole-library tooling (included) | `2a3ca33` |
| [#7](https://github.com/videoP/DinurdoJK/pull/7) | 20:51:01 | Local emote playback and UI | `cc7fbf8` |
| [#8](https://github.com/videoP/DinurdoJK/pull/8) | 20:51:17 | Offline visual gallery | `e851364` |
| [#9](https://github.com/videoP/DinurdoJK/pull/9) | 20:51:34 | Unreal player models and idle effects | `1f9893e` |
| [#10](https://github.com/videoP/DinurdoJK/pull/10) | 20:51:53 | Mapping prop tools | `bea7a99` |
| [#11](https://github.com/videoP/DinurdoJK/pull/11) | 21:08:52 | CUE4Parse exporter | `8c4b133` |

## Conflict resolutions and integration fixes

- #1, #2, #4: workspace member conflicts resolved by retaining BlueWater FX
  and adding all three new crates. Existing build profiles remain intact.
- #4, #11: ignore rules combined, preserving Anny authoring-tool exceptions
  and adding generated emote/exporter output exclusions.
- #7: combined player asset fields/initializers for body shapes, Source jiggle
  palettes, and emote ground-contact caches. Retained seven preview anchors,
  the advancing jiggle simulation clock, body sliders, and animation smoothing.
  Added emote playback, effects, weapon holstering, and emote identity in the
  preview cache key. Existing walking previews and continuous timing take
  precedence over the PR's periodic idle fidgets and 16/33-ms clock quantization;
  the existing frontend render cap supplies animation updates.
- #9: retained Source jiggle bindings beside the new format-neutral rig
  geometry. Moved existing body-shape application into the shared model-finishing
  path and retained idle effects. Source body-shape fingerprints still use the
  MDL; Unreal models use their first mesh if a body-shape sidecar exists.
- External emote poses now pass through the shared mesh binding path, retaining
  body-shape deformation and appended Source jiggle matrices. Ordinary gameplay
  and ragdolls retain the original animation binding path.
- Unreal `SourceBone` construction explicitly sets `jiggle: None`, matching
  the existing skeleton type without inventing procedural Unreal jiggle data.
- Mapping smoke-test Cargo fallback uses `--profile fast`.
- Updated three new presenter test calls for the existing movement-mode
  argument. Added a regression assertion that external emote poses retain
  extra Source jiggle palette entries.
- Assigned the three new emote settings unused archive positions 27346–27348.
  Their original PR positions collided with existing settings (29 and 389).
  Existing archive positions were not reordered.

The CUE4Parse submodule was initialized at its PR-pinned commit
`ed9897a1a3a8dba84a4b6d2119846e0502d74f19`.

## Validation

- All 11 fetched PR heads are ancestors of the integration branch. `main`
  still points to the starting commit. `git diff --check` passes.
- New feature crate tests (`jka-ue-assets`, `jka-ue-runtime`, `jka-emotes`):
  **91 passed, 11 ignored**. Asset-dependent opt-in tests were not supplied
  real exports/libraries; some tests return early without those inputs.
- Python synthetic skin packaging: **2 passed**. End-to-end mapping import,
  Radiant pack, and BSP packaging: **1 passed**, using `target/fast/djk-ue-import.exe`.
- Broader `jka-assets` unit tests: **184 passed, 3 failed, 9 ignored**. Failures:
  `skin::tests::comma_parser_and_off_rules_match_register_individual_skin`,
  `source1::tests::parses_compiled_jigglebone_rule`, and
  `source1::tests::refuses_out_of_bounds_jiggle_rule`. Both affected source
  files are byte-for-byte unchanged from the starting commit. These failures
  are outside the integration changes and were not altered.
- Full client suite before the final archive-position fix: **1277 passed,
  25 failed, 80 ignored**. All player-presenter tests, including the new
  emote/Source palette regression, passed. Failures include Windows event-loop
  recreation, stale configuration baselines/counts, material/FX assertions,
  and ocean shader/layout expectations. The archive-position failure exposed
  the new setting collisions fixed above. Static verification confirms each
  new rank is used exactly once; the starting code has other duplicate ranks.
  The count assertion expects 425 archived settings, whereas the combined
  registry has 646 (only three were added by these PRs).
  The starting code was not rebuilt separately, so the remaining failures
  are not claimed to have been exhaustively reproduced on the baseline.
  Generated gamma-audit test fixtures are ignored and were not deleted.
- Final Rust workspace rebuild after the archive-position fix:
  **compiled successfully** with `build.ps1 -Fast` (fast profile, 1m 23s,
  warnings remain). The build script exited unsuccessfully during the runtime
  PK3 copy described below. The latest executable was copied to `target/release`.

`build.ps1 -Fast` compiled the complete Rust workspace successfully
and copied the game executable into `target/release`. The script then encountered
access denied while copying `base/dinurdo-assets.pk3`; its destination already
matches the source SHA-256. This is a runtime asset-copy failure, not a compiler
error. No running application was stopped to release the file.

The C# exporter and its pinned CUE4Parse dependency target .NET 10. This machine
has only SDK 5.0.408, so exporter compilation cannot be verified here. No SDK
upgrade was performed. Real game exports, emote libraries, and visual gameplay
validation require separately supplied assets; synthetic and unit tests do not
establish their visual correctness.
