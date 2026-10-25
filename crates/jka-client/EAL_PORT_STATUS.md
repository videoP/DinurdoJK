# Authored EAL acoustics

Windows EAL rendering uses statically linked EalMan v3 and source-built OpenAL
Soft **1.25.2**, pinned to the versions/settings in JAEnhanced PR #10 at
`a5d7c7d307b541a3387f760553392834d9e8ced0`. The runtime is
`OpenALSoft.x86_64.dll`. There is no EalMan DLL or SDL2 dependency.

## Use

`s_eal` is archived and defaults to **0**. Enable with `s_eal 1`, or
**Audio > Map Acoustics > EAL map audio**. Changes apply live, including to
continuing loops; `snd_restart` is unnecessary. Disable with `s_eal 0`.
Maps without `eagle/<map>.eal` retain ordinary audio. Local/devmap sessions
load the dataset after assigning the server map name.

Use `soundinfo` to check the dataset, listener environment, renderer errors,
source/room/filter/diffraction/directionality counts, HRTF, scene latency,
maximum render time, queue drops and underruns. Counters describe the most
recent worker block, so a filter/diffraction count of zero can be normal for
unobstructed sources. Test on `devmap mp/ffa3`: fire a weapon, cross room
boundaries and listen around corners, then compare live with `s_eal 0`.

## Signal ownership

The former summed mono wet bus has been replaced with a **per-source EAX
scene**. Each world sound retains its own PCM stream, source position,
playback clock, model and EAX routing. Music and local/UI one-shots retain
their ordinary paths. Listener-owned world sounds can receive room effects.
Identical looping assets on different emitters remain separate in EAL mode.

For an EAL-owned source, OpenAL renders direct audio, authored LF/HF
obstruction/occlusion and reverb together. Its legacy/Steam output fades out
once the corresponding native output is available. Steam geometry filtering
is not applied to the OpenAL signal. A continuing fallback voice advances
its clock so disabled/failed/late native rendering can restore ordinary sound.
Both paths meet in Rodio/CPAL, the only physical output device.

EAL works with Steam Audio disabled. Its independent archived `s_ealHrtf`
preference (default **0**) controls OpenAL Soft's own HRTF, also available as
**Audio > Map Acoustics > OpenAL headphone HRTF**. The menu and `soundinfo`
report the device's actual HRTF state, including refused requests. Other
world audio uses Steam's HRTF when enabled and available. Their HRTF models may
sound different. Neither sample identity with Creative hardware nor identical
Steam/OpenAL output is claimed.

## Authored processing

- Game coordinates `[x,y,z]` become Eagle `[x,z,y]` for EalMan and OpenAL
  `[x,z,-y]` for rendering. Both listener and source use the same transform.
- EalMan uses the authored BSP/material matrix and diffraction boxes, with
  diffraction enabled and its one-unit partition tolerance. LF ratios,
  occlusion room/direct ratios and obstruction affect EAX frequency filters.
- Explicit source models are matched by associated sound path or model name;
  valid direct/room HF levels, exclusion and source cones are retained. The
  authoring template is not blindly applied to every engine-created sound.
  Invalid source scalar values are bounded; known NaNs in retail source
  models use EAX defaults for the affected field.
- Independent EAX reverbs use the complete authored room presets. The
  listener room and two nearby aperture rooms occupy EAX4's three reverb
  slots (0, 2, 3). Slot 1 is reserved for legacy chorus by OpenAL Soft.
- Nearest authored aperture endpoints determine reverb angular spread and
  early/late pan in listener coordinates. Room changes retain stable slots
  where possible. Cross-room sends receive the PR's exclusion attenuation.
- As with the PR's finite EAX room budget, an additional room without an
  active slot falls back to the primary room. Dry audio remains present.
  This is a documented routing approximation under simultaneous room limits.
- Up to 96 native sources (32 one-shots plus up to 64 loops) are retained.
  Excess loop emitters retain up to 32 legacy per-sfx merged fallback channels,
  reported in soundinfo, so splitting loops does not remove their audio.
  Retiring-source pressure queues registrations; if the bounded queue fills,
  an unregistered voice retains ordinary audio and increments diagnostics.
- Source pitch, volume and distance gains remain controlled by JKA. OpenAL
  distance rolloff/doppler are disabled to avoid applying those twice. Loops
  retain normal pitch across demo timescale changes; one-shots follow their
  presentation speed. Pause and loop offsets are preserved.

One worker owns native creation, processing and destruction. Fixed SPSC rings
exchange 256-frame scenes with two transport blocks plus one native input
preroll block (16 ms at 48 kHz). Normal mode uses `AL_SOFT_callback_buffer`
with a bounded preallocated per-source PCM FIFO. The callbacks execute
synchronously inside the worker's loopback render; no additional output
device or audio thread is introduced. A completed source returns EOF and
remains attached until OpenAL finishes playing its prefetched final samples.
Without the callback extension, normal mode falls back to the original queue
transport. `s_ealDebug 7` forces callbacks; `9` forces the old queue for A/B.
`soundinfo` identifies the actual transport and cumulative missing/dropped
callback sample counts. Those counters survive source retirement and reset
on a diagnostic mode switch or scene recreation. A bounded FIFO cannot
overwrite unread PCM: a full-block drop is counted explicitly.
Physical playback never waits for native DSP; late results are discarded.
Offline export waits for completion while retaining the same fixed delay.
AL/ALC errors or nonfinite output disable the scene and restore ordinary audio.
Rust validates dataset bounds/tree/material indices before native loading;
native list/array allocations are also bounded. JKA EAL 3.0 is supported by
the native scene; EAL 2.1 parsing remains available for validation, but that
version does not silently become an approximate EAL3 render.

## Build and distribution

`build.ps1 -Fast` builds/reuses OpenAL from the pinned local archive, statically
builds EalMan and stages the game into `target/release`. Include the four
app-local MSVC DLLs (`msvcp140`, `msvcp140_atomic_wait`, `vcruntime140`,
`vcruntime140_1`), the OpenAL DLL and both licenses under `licenses/`.
Release packaging and signed updates explicitly allowlist those files.
Older updater binaries need one manual upgrade before accepting the expanded
signed payload. The original Creative OpenAL DLL cannot provide this scene.

## Validation

Generated EAL fixtures exercise spectral LF/HF attenuation, diffraction within
one room, distinct room decay, aperture directionality, multiple identical
source paths and live HRTF changes against the actual pinned runtime.
Transport regressions cover late clock synchronization, acknowledged handover,
final partial PCM, bounded capacity, fallback, toggles and source generations.
Retail dataset grid rendering and local-map/restart tests supplement these.
The callback follow-up focused run passed **56 tests**: 46 regular audio/
parser/transport/config/updater tests, nine native tests, and one local-game
integration test. Default mode 0 matched forced callback mode 7 within
0.000001 per sample, with no ongoing FIFO underfeeds, dropped samples or
source restarts in the continuous-tone test. Short sources of
1/64/255/256/257/513 samples matched a continuously zero-fed reference exactly
at both 44.1 and 48 kHz after the EOF fixes, retaining every final marker.
EOF is preceded by terminal silence so OpenAL cannot stretch the last
nonzero sample across its resampler read window. Mode switches and actual
device HRTF state passed; the local-game test retained independent EAL HRTF
with Steam disabled across `snd_restart` and a configuration round trip.
The native grid test rendered all **58 retail JKA EAL datasets** without
property errors. Frequency tests measured LF energy retention of 0.979 for a
wall with zero LF attenuation, versus HF energy retention of 0.00000386;
a solid LF wall retained 0.0001. Diffraction reduced the test HF energy to
0.0547 of its unobstructed result. Separate room decay and aperture panning
were checked against rendered PCM, including identical sound paths in two rooms.

The 96-source smoke covered 44.1/48 kHz and live HRTF changes. Initialization/
HRTF reconfiguration can exceed one block deadline; these are smoke results,
not a game frame-time guarantee. A paced single-source worker run had zero
underruns and drops. Current logs: `.codex-eal-callback-results.log` and
`.codex-eal-callback-tests-build.log` in the workspace root. Earlier native
validation is in `.codex-eal-scene-results.log`. Fixture-dependent
stock-library/emote tests remain ignored in the selected run.

Earlier parser validation accepted all 58 retail JKA datasets and 33 supplied
EalMan fixtures, and matched 38,599 listener/source matrix queries with
`EMFLAG_NODIFFRACTION`. Those earlier oracle checks do not by themselves
validate the new native scene or listening equivalence. In-game listening and
representative frame-time checks are still appropriate before defaulting EAL on.

References: vendored sources in `vendor/ealman`, `vendor/openal-soft`,
https://github.com/JAEnhanced/JAEnhanced/pull/10,
https://github.com/senneschall/EalMan, https://github.com/kcat/openal-soft.

The callback follow-up `build.ps1 -Fast` completed and staged successfully on
2026-10-24. EXE/PDB, OpenAL and all four CRT DLL SHA-256 values match
`target/fast`; both licenses are present. No cache cleanup was performed.
Build log: `.codex-eal-callback-build.log`. Earlier import inspection confirmed
neither the EXE nor OpenAL
imports SDL2.dll or EalMan.dll; OpenAL reports file/product version 1.25.2.
