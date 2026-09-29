---
name: bench-map
description: Launch a map in the DinurdoJK client, teleport to a view spot with setviewpos, measure FPS/perf with perfsample, take screenshots and quit - all scripted. Use for any FPS-at-a-spot investigation, renderer before/after comparison, or to reproduce a spot the user reports from `viewpos`.
---

# Bench a map: launch, teleport, measure FPS, screenshot

Everything runs through `scripts/bench-map.sh`, which launches the release
client with scripted console commands, prints the key log lines, and restores
the user's config afterwards.

## Steps

1. **Build** (about 30 s, fast profile):
   `powershell.exe -NoProfile -ExecutionPolicy Bypass -File build.ps1 -Fast`
   The bench uses `target/release/DinurdoJK.exe`; the script copies it to a
   scratch folder, so a game the user has open is not disturbed.
2. **Run**:
   `scripts/bench-map.sh <timeout-seconds> <MAP> [+command ...]`
3. **Read the output** (see below), compare at least 3 launches, and check the
   `view=` field of each sample.

Typical FPS measurement at a spot:

```
scripts/bench-map.sh 400 AMJH3TE +set r_perftrace 1 \
  +setviewpos 140 -458 -5 71 15 +perfsample 3 settle +perfsample 5 run1 +quit
```

Add `+screenshot` before a `+perfsample` for an image, `+set r_gpuTimings 1`
for GPU pass timings.

## Console commands available to scripts

- `setviewpos x y z yaw [pitch]`: port of OpenJK TeleportPlayer (feet origin,
  eye = feet + 36 as spectator). **The values are not what `viewpos` prints:**
  to reproduce a `viewpos` spot use z = printed eye z - 37 (eye 32 -> z -5).
- `viewpos`: prints the view origin in the format users report spots in.
- `perfsample <seconds> [label]`: holds the command buffer, then logs one line
  `[JKA PERF SAMPLE] label=... fps_avg fps_min fps_max cpu_frame_avg
  gpu_frame_avg ... view=(x y z) yaw pitch`. Use a short `settle` sample first.
- `screenshot`: writes `shot<date>.jpg` into `target/release/base/screenshots`.
- `trace`: runs the surface inspector; the result is logged as
  `SURFACE INSPECTOR:` lines.
- `quit`: exit. Always end scripted runs with it.
- Commands queued after a map load wait for the load (console map barrier), so
  `+setviewpos` right after `--map` is safe.
- `+set r_perftrace 1` adds `[JKA PERF]` lines: `encode=`, `world_batches[pvs=
  frustum_reject= encoded= multidraw_groups= multidraw_batches=]`, and more.
  `r_gpuTimings 1` adds `[JKA PERF GPU]` per-pass times (world, post, ...).

## Reading results

- FPS: the `[JKA PERF SAMPLE]` lines. `cpu_frame_avg` vs `gpu_frame_avg` shows
  which side limits (needs `r_gpuTimings 1` for the GPU number).
- CPU cost is dominated by **draw count**: `encoded=` draws cost about
  1.1 us each in wgpu's `encoder.finish()` plus about 0.5 us recording. To find
  why draws are not merging, temporarily count them by reason and remove the
  counters afterwards.
- Log file: `<scratch>/run/latest.log` (path printed by the script).

## Rules (each one cost a real mistake)

- **Config safety.** Any `set` rewrites `target/release/base/DinurdoJK.cfg`.
  The script backs it up and restores it on exit. `r_backend` and
  `r_reflectionQuality` are restart-latched: change them by passing a cfg file
  through `JKA_BENCH_CFG=<file>`, not with `+set`. After any interrupted run,
  diff the cfg against the backup the script printed.
- **Screenshots.** Never move or delete screenshots by "newest" or by name; a
  stalled run takes none and the user's own shots share the folder. The script
  only moves files newer than a marker created at start.
- **Mouse drift.** If the user touches the mouse during a run the view shifts
  (yaw/pitch in the `view=` field differ from the command). Discard and rerun
  such samples; they skew FPS and screenshot diffs.
- **Variance.** Single runs vary +/-15%; a cold shader/asset cache can stall or
  slow the first launch (use a long timeout, e.g. 400). Compare 3 launches.
- **Other sessions.** Other Claude sessions may be editing `renderer.rs`; check
  mtimes and make targeted edits.

## Verifying a rendering change is image-identical

Take a `+screenshot` at the same `setviewpos` before and after, then diff with
Pillow/numpy (mask the FPS text in the top-right corner). Identical views give
a mean difference under about 0.03; anything larger is either a real change or
a drifted view (check `view=`).

## Known reference (AMJH3TE, Legacy renderer, Vulkan)

Spot `setviewpos 140 -458 -5 71 15` (`viewpos` shows 140 -458 32): about
610-665 FPS after the 2026-09-28 draw-batching work (was 281). OpenJK does
about 290 there. DX12 (`r_backend "dx12"` in the cfg) runs at about 390 FPS.
